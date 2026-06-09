//! Process management — M10.
//!
//! ## What this module provides
//!
//! * [`Process`] — one address-space unit (kernel view of a userspace process).
//! * [`PROCESS_TABLE`] — global table of all live processes (max 256).
//! * [`spawn`] — load an ELF, create a new user address space, and schedule the
//!   first thread.
//! * [`exit`] — tear down a process and reclaim its physical frames.
//! * [`kill`] — send SIGKILL (immediate `exit`) to any process by PID.
//!
//! ## Memory layout for user processes
//!
//! ```text
//! 0x0000_0000_0040_0000  ..  ELF load base (2 MiB)
//! 0x0000_7FFF_FFFF_0000  ..  user stack top  (grows down, 64 KiB initial)
//! ```
//!
//! The kernel's HHDM (0xffff_8000_…) and its own page tables are NOT mapped
//! into the user PML4 — enforced by starting user entries at PML4[0] while
//! the kernel occupies the top half (PML4[256+]).

pub mod dl;
pub mod elf;
pub mod posix_trampolines;

use alloc::alloc::{alloc, dealloc, Layout};
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

use crate::mm::{frame_allocator, paging};

// ── PID to launch via the "user-init" kernel task ─────────────────────────────

/// PID stored here before `schedule_user_launch` spawns the kernel task.
static PENDING_INIT_PID: AtomicU32 = AtomicU32::new(1);

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum simultaneous live processes.
pub const MAX_PROCS: usize = 256;

/// ELF load base virtual address (user space).
pub const USER_ELF_BASE: u64 = 0x0000_0000_0040_0000; // 4 MiB

/// User stack top (grows downward; bottom = TOP − STACK_SIZE).
///
/// x86_64 uses a 47-bit canonical-low address. aarch64's bring-up MMU configures
/// a 39-bit TTBR0 VA window (T0SZ=25 → max user VA 512 GiB), so the ARM stack top
/// is placed just under that boundary instead.
#[cfg(not(target_arch = "aarch64"))]
pub const USER_STACK_TOP:  u64  = 0x0000_7FFF_FFFF_0000;
#[cfg(target_arch = "aarch64")]
pub const USER_STACK_TOP:  u64  = 0x0000_007F_FFF0_0000; // ~512 GiB − 1 MiB (39-bit VA)
pub const USER_STACK_SIZE: usize = 4 * 1024 * 1024; // 4 MiB
const SYSCALL_STACK_SIZE: usize = 64 * 1024;
const XSTATE_SIZE: usize = 4096;

// ── Process state ─────────────────────────────────────────────────────────────

/// Lifecycle state of a process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcState {
    /// Slot is free.
    Dead,
    /// Process is runnable (or currently running on a CPU).
    Running,
    /// Blocked waiting for an event (sleep, waitpid, IPC recv).
    Blocked,
    /// `exit()` called; waiting for parent to `waitpid()`.
    Zombie(i32),
}

// ── Per-process saved register state (x86_64 SYSCALL/SYSRET convention) ───────

/// Minimal saved CPU context for a user thread.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct UserRegs {
    pub rip:    u64,
    pub rsp:    u64,
    pub rflags: u64,
    pub rax: u64, pub rbx: u64, pub rcx: u64, pub rdx: u64,
    pub rsi: u64, pub rdi: u64, pub rbp: u64,
    pub r8:  u64, pub r9:  u64, pub r10: u64, pub r11: u64,
    pub r12: u64, pub r13: u64, pub r14: u64, pub r15: u64,
}

#[repr(C, align(64))]
#[derive(Clone, Copy)]
struct XStateBuf([u8; XSTATE_SIZE]);

impl Default for XStateBuf {
    fn default() -> Self {
        let mut buf = Self([0; XSTATE_SIZE]);
        // fxsave64 area layout (Intel SDM Vol.1 §13.4):
        //   offset  0: FCW  (2 bytes) — x87 FPU control word
        //   offset 24: MXCSR (4 bytes) — SSE control/status
        //
        // Default FCW = 0x037F: all x87 exceptions masked, 64-bit precision,
        // round-to-nearest. Without this, new threads start with FCW=0
        // (24-bit precision + all exceptions unmasked).
        buf.0[0] = 0x7F;
        buf.0[1] = 0x03;
        // Default MXCSR = 0x1F80: all SSE floating-point exceptions masked,
        // round-to-nearest, no flush-to-zero. Without this, new threads start
        // with MXCSR=0 (all FP exceptions unmasked), which can trigger #XM
        // faults on any SSE FP operation that produces a special value.
        buf.0[24] = 0x80;
        buf.0[25] = 0x1F;
        buf
    }
}

// ── Process descriptor ────────────────────────────────────────────────────────

/// Kernel-side process descriptor.
pub struct Process {
    /// Process ID (1-based; 0 = kernel/PID-0).
    pub pid:     u32,
    /// Physical address of this process's PML4 (page table root).
    pub pml4_phys: u64,
    /// Saved registers (updated on every kernel entry, restored on exit).
    pub regs:    UserRegs,
    /// Current lifecycle state.
    pub state:   ProcState,
    /// Exit code set by `sys_exit`.
    pub exit_code: i32,
    /// Base of this process syscall kernel stack.
    syscall_stack_base: *mut u8,
    /// Top of this process syscall kernel stack.
    syscall_stack_top: u64,
    /// XSAVE/FXSAVE image for this process.
    xstate: XStateBuf,
    /// True when this slot represents a thread (shares pml4 with parent).
    /// Threads do not own their PML4 — `exit` will skip `free_user_pml4`.
    pub is_thread:  bool,
    /// For threads: the PID of the owning process.  0 for standalone processes.
    pub parent_pid: u32,
    // ── Phase 53: per-process CPU accounting ──────────────────────────────────
    /// Total APIC timer ticks this process has consumed.
    pub cpu_ticks:   u64,
    /// Remaining time-slice ticks before a forced preemption.
    pub slice_left:  u32,
    // ── Phase 55: signal state ───────────────────────────────────────────────
    /// Pending signal bitmask (bit N = signal N+1 pending).
    pub pending_sigs: u32,
    /// Signal mask (blocked signals bitmask).
    pub sig_mask:     u32,
    /// Per-signal handlers: 0 = default, 1 = ignore, else user VA of handler.
    pub sig_handlers: [u64; 32],
    /// x86_64 FS base MSR (TLS pointer) for this thread.  Saved/restored
    /// across user entries so each pthread sees its own TLS.  0 means the
    /// FS base has not yet been initialised (triggers per-thread bootstrap
    /// on the first syscall from this thread).
    pub fs_base: u64,
    /// Errno value to deliver into SD_ERRNO (SYSDATA_VA+32) just before this
    /// thread's next SYSRET.  Written to user memory after the CR3 switch so
    /// that intermediate thread switches cannot overwrite it.  0 = nothing.
    pub errno_to_deliver: u32,
    /// True when this thread was preempted by the APIC timer ISR (not at a
    /// SYSCALL boundary).  `enter_user_by_pid_noreturn` must use IRETQ (not
    /// SYSRETQ) to restore all registers — including RCX, R11, and the exact
    /// RFLAGS — instead of the SYSCALL-convention values.  Cleared whenever
    /// `save_return_context` is called (syscall-based yield).
    pub preempted_by_timer: bool,
    /// Base virtual address of this thread/process's user-space stack.
    /// Used by sys_pthread_attr_getstack so the Dart VM can validate stack bounds.
    pub user_stack_base: u64,
    /// Size in bytes of this thread/process's user-space stack.
    pub user_stack_size: u64,
    /// CPU core index that currently executes this process context (None if idle/blocked).
    pub current_cpu: Option<u32>,
}

impl Process {
    const fn empty() -> Self {
        Self {
            pid:       0,
            pml4_phys: 0,
            regs:      UserRegs {
                rip: 0, rsp: 0, rflags: 0,
                rax: 0, rbx: 0, rcx: 0, rdx: 0,
                rsi: 0, rdi: 0, rbp: 0,
                r8:  0, r9:  0, r10: 0, r11: 0,
                r12: 0, r13: 0, r14: 0, r15: 0,
            },
            state:     ProcState::Dead,
            exit_code: 0,
            syscall_stack_base: core::ptr::null_mut(),
            syscall_stack_top: 0,
            xstate: XStateBuf([0; XSTATE_SIZE]),
            is_thread:  false,
            parent_pid: 0,
            cpu_ticks:   0,
            slice_left:  10,  // default 10-tick quantum
            pending_sigs: 0,
            sig_mask:     0,
            sig_handlers: [0u64; 32],
            fs_base:      0,
            errno_to_deliver: 0,
            preempted_by_timer: false,
            user_stack_base: 0,
            user_stack_size: 0,
            current_cpu: None,
        }
    }
}

/// Persist the FS base for `pid` (called on arch_prctl(ARCH_SET_FS) and on
/// the FS-bootstrap path) so it is restored on the next user entry.
pub fn set_proc_fs_base(pid: u32, fs: u64) {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx_of(pid)] };
    if p.pid == pid { p.fs_base = fs; }
}

/// Read the saved FS base for `pid`, or 0 if not set / pid invalid.
pub fn get_proc_fs_base(pid: u32) -> u64 {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &PTABLE[idx_of(pid)] };
    if p.pid == pid { p.fs_base } else { 0 }
}

/// Return the (base, size) of the user-space stack for `pid`.
/// Returns (0, 0) if `pid` is unknown or the slot has no recorded stack.
pub fn get_user_stack_bounds(pid: u32) -> (u64, u64) {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &PTABLE[idx_of(pid)] };
    if p.pid == pid {
        (p.user_stack_base, p.user_stack_size)
    } else {
        (0, 0)
    }
}

/// Returns true if `pid` is a pthread (clone-thread) of another process.
/// Used by the syscall FS-bootstrap to skip auto-assigning a fake FS base
/// for new threads — pthread runtimes set their own FS via arch_prctl.
pub fn is_thread(pid: u32) -> bool {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &PTABLE[idx_of(pid)] };
    p.pid == pid && p.is_thread
}

/// Walk up the thread parent chain to resolve the root thread group leader.
pub fn get_group_leader(pid: u32) -> u32 {
    let _g = PTABLE_LOCK.lock();
    get_group_leader_locked(pid)
}

pub(crate) fn get_group_leader_locked(pid: u32) -> u32 {
    let mut curr = pid;
    loop {
        let p = unsafe { &PTABLE[idx_of(curr)] };
        if p.pid == curr && p.is_thread && p.parent_pid != 0 {
            curr = p.parent_pid;
        } else {
            break;
        }
    }
    curr
}

// ── Global process table ──────────────────────────────────────────────────────

// Safety: all mutations are protected by `PTABLE_LOCK`.
pub(crate) static mut PTABLE: [Process; MAX_PROCS] = {
    // SAFETY: Process::empty() is a const fn, array is zero-initialised.
    [const { Process::empty() }; MAX_PROCS]
};
pub struct PTableGuard {
    is_outer: bool,
}

pub struct PTableLock {
    inner: Mutex<()>,
    holder: AtomicU32,
}

impl PTableLock {
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(()),
            holder: AtomicU32::new(0xFFFF_FFFF),
        }
    }

    pub fn lock(&self) -> PTableGuard {
        let my_cpu = crate::arch::smp::this_cpu().cpu_id;
        if self.holder.load(Ordering::Acquire) == my_cpu {
            unsafe {
                PTABLE_LOCK_RECURSION[my_cpu as usize] += 1;
            }
            PTableGuard { is_outer: false }
        } else {
            let g = self.inner.lock();
            self.holder.store(my_cpu, Ordering::Release);
            unsafe {
                PTABLE_LOCK_RECURSION[my_cpu as usize] = 1;
                PTABLE_LOCK_GUARD = Some(core::mem::transmute(g));
            }
            PTableGuard { is_outer: true }
        }
    }

    pub fn try_lock(&self) -> Option<PTableGuard> {
        let my_cpu = crate::arch::smp::this_cpu().cpu_id;
        if self.holder.load(Ordering::Acquire) == my_cpu {
            unsafe {
                PTABLE_LOCK_RECURSION[my_cpu as usize] += 1;
            }
            Some(PTableGuard { is_outer: false })
        } else {
            if let Some(g) = self.inner.try_lock() {
                self.holder.store(my_cpu, Ordering::Release);
                unsafe {
                    PTABLE_LOCK_RECURSION[my_cpu as usize] = 1;
                    PTABLE_LOCK_GUARD = Some(core::mem::transmute(g));
                }
                Some(PTableGuard { is_outer: true })
            } else {
                None
            }
        }
    }
}

impl Drop for PTableGuard {
    fn drop(&mut self) {
        if self.is_outer {
            let my_cpu = crate::arch::smp::this_cpu().cpu_id;
            unsafe {
                PTABLE_LOCK_RECURSION[my_cpu as usize] = 0;
                PTABLE_LOCK.holder.store(0xFFFF_FFFF, Ordering::Release);
                PTABLE_LOCK_GUARD = None;
            }
        } else {
            let my_cpu = crate::arch::smp::this_cpu().cpu_id;
            unsafe {
                if PTABLE_LOCK_RECURSION[my_cpu as usize] > 0 {
                    PTABLE_LOCK_RECURSION[my_cpu as usize] -= 1;
                }
            }
        }
    }
}

static mut PTABLE_LOCK_GUARD: Option<spin::MutexGuard<'static, ()>> = None;
static mut PTABLE_LOCK_RECURSION: [u32; 64] = [0; 64];

pub(crate) static PTABLE_LOCK: PTableLock = PTableLock::new();

/// Bitmask of processes with a pending wake-up to be processed lock-free.
pub static PENDING_WAKES: [AtomicU32; 8] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];

/// Safely queue a wake-up for the given PID. Deadlock-free from any context.
pub fn wake_process(pid: u32) {
    let idx = idx_of(pid);
    let word = idx / 32;
    let bit = idx % 32;
    if word < 8 {
        PENDING_WAKES[word].fetch_or(1 << bit, Ordering::SeqCst);
    }
    handle_pending_wakes_try();
}

/// Try to process pending wake-ups if PTABLE_LOCK is not held.
pub fn handle_pending_wakes_try() {
    let mut any_pending = false;
    for word in 0..8 {
        if PENDING_WAKES[word].load(Ordering::Acquire) != 0 {
            any_pending = true;
            break;
        }
    }
    if !any_pending { return; }

    if let Some(_g) = PTABLE_LOCK.try_lock() {
        let mut should_broadcast = false;
        for word in 0..8 {
            let mut active_mask = PENDING_WAKES[word].swap(0, Ordering::SeqCst);
            while active_mask != 0 {
                let bit = active_mask.trailing_zeros();
                active_mask &= !(1 << bit);
                let idx = word * 32 + bit as usize;
                let p = unsafe { &mut PTABLE[idx] };
                if p.pid != 0 && p.state == ProcState::Blocked {
                    let old_state = p.state;
                    p.state = ProcState::Running;
                    if old_state != ProcState::Running {
                        should_broadcast = true;
                    }
                }
            }
        }
        drop(_g);
        if should_broadcast {
            crate::arch::smp::broadcast_resched_ipi();
        }
    }
}

/// Next PID to hand out (starts at 1; PID 0 is the kernel).
static NEXT_PID: AtomicU32 = AtomicU32::new(1);
// static CURRENT_PID: AtomicU32 = AtomicU32::new(0);

// ── PID helpers ───────────────────────────────────────────────────────────────

fn alloc_pid() -> Option<u32> {
    // Linear scan — fine for MAX_PROCS = 256.
    for _ in 0..MAX_PROCS as u32 {
        let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);
        let idx = pid as usize % MAX_PROCS;
        let _guard = PTABLE_LOCK.lock();
        // SAFETY: lock held.
        if unsafe { PTABLE[idx].state } == ProcState::Dead {
            return Some(pid);
        }
    }
    None
}

pub fn idx_of(pid: u32) -> usize {
    pid as usize % MAX_PROCS
}

fn alloc_syscall_stack() -> Option<(*mut u8, u64)> {
    // Back syscall stacks with raw physical frames mapped through the HHDM,
    // bypassing the kernel heap entirely. The linked-list allocator becomes
    // pathologically fragmented after dlopen of libflutter_engine.so (lots of
    // small `Vec` reallocations), which causes 8-KiB requests to fail even
    // with >15 MiB nominally free. Frames give us a guaranteed, contiguous,
    // page-aligned span — perfect for a kernel stack.
    const STACK_FRAMES: usize = (SYSCALL_STACK_SIZE + 4095) / 4096;
    let phys = crate::mm::frame_allocator::alloc_contiguous_frames(STACK_FRAMES)?;
    let base = (phys + crate::mm::frame_allocator::hhdm_offset()) as *mut u8;
    let top = unsafe { base.add(STACK_FRAMES * 4096) } as u64;
    Some((base, top))
}

fn free_syscall_stack(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    const STACK_FRAMES: usize = (SYSCALL_STACK_SIZE + 4095) / 4096;
    let virt = ptr as u64;
    let hhdm = crate::mm::frame_allocator::hhdm_offset();
    if virt < hhdm {
        return; // not an HHDM-backed stack — leave it alone
    }
    let phys_base = virt - hhdm;
    for i in 0..STACK_FRAMES {
        crate::mm::frame_allocator::free_frame(phys_base + (i as u64) * 4096);
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Bootstrap values passed to a new process entry (SysV ABI: rdi, rsi, rdx).
#[derive(Clone, Copy, Default)]
pub struct SpawnBootstrap {
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub parent_pid: u32,
}

/// Spawn a new process from an ELF image in memory.
///
/// Returns the new PID, or an error string.
pub fn spawn(elf_bytes: &[u8], name: &str) -> Result<u32, &'static str> {
    spawn_with_bootstrap(elf_bytes, name, SpawnBootstrap::default())
}

/// Spawn a new process with optional bootstrap registers and parent linkage.
pub fn spawn_with_bootstrap(
    elf_bytes: &[u8],
    name: &str,
    bootstrap: SpawnBootstrap,
) -> Result<u32, &'static str> {
    let pid = alloc_pid().ok_or("process table full")?;
    let (sys_stack_base, sys_stack_top) = alloc_syscall_stack().ok_or("OOM: syscall stack")?;

    // Allocate a fresh PML4 for this process.
    let pml4_phys = paging::alloc_user_pml4().ok_or("OOM: pml4")?;

    // Parse the ELF and load segments into the new address space.
    let entry = elf::load(elf_bytes, pml4_phys)?;

    // Map a user stack.
    let stack_bottom = USER_STACK_TOP - USER_STACK_SIZE as u64;
    let stack_pages = USER_STACK_SIZE / 4096;
    for i in 0..stack_pages {
        let virt = stack_bottom + (i * 4096) as u64;
        let phys = frame_allocator::alloc_frame().ok_or("OOM: stack")?;
        // Zero the stack page.
        unsafe {
            core::ptr::write_bytes(
                (phys + frame_allocator::hhdm_offset()) as *mut u8,
                0,
                4096,
            );
        }
        unsafe {
            paging::map_user_page(pml4_phys, virt, phys)?;
        }
    }

    // Map POSIX trampoline + sysdata pages so glibc symbols resolve correctly.
    // These are x86_64 glibc/musl shims; the aarch64 bring-up init is a bare EL0
    // program with no libc, so they are skipped on ARM (follow-on once an ARM
    // userland links a C library).
    #[cfg(not(target_arch = "aarch64"))]
    posix_trampolines::map_system_pages(pml4_phys)?;

    let idx = idx_of(pid);
    let mut regs = UserRegs::default();
    regs.rip    = entry;
    regs.rsp    = USER_STACK_TOP - 8; // leave one guard word
    regs.rdi    = bootstrap.rdi;
    regs.rsi    = bootstrap.rsi;
    regs.rdx    = bootstrap.rdx;
    regs.rflags = 0x0202;             // IF=1, reserved=1

    {
        let _g = PTABLE_LOCK.lock();
        // SAFETY: lock held, idx is valid.
        let p = unsafe { &mut PTABLE[idx] };
        p.pid        = pid;
        p.pml4_phys  = pml4_phys;
        p.regs       = regs;
        p.state      = ProcState::Running;
        p.exit_code  = 0;
        p.syscall_stack_base = sys_stack_base;
        p.syscall_stack_top = sys_stack_top;
        p.xstate = XStateBuf::default();
        p.is_thread  = false;
        p.parent_pid = bootstrap.parent_pid;
        p.fs_base    = 0;
        // Record user stack bounds so pthread_attr_getstack can return them.
        p.user_stack_base = USER_STACK_TOP - USER_STACK_SIZE as u64;
        p.user_stack_size = USER_STACK_SIZE as u64;
        p.current_cpu        = None;
        p.cpu_ticks          = 0;
        p.slice_left         = 10;
        p.pending_sigs       = 0;
        p.sig_mask           = 0;
        p.sig_handlers       = [0u64; 32];
        p.errno_to_deliver   = 0;
        p.preempted_by_timer = false;
    }

    log::info!(
        "[Process] Spawned '{}' pid={} entry={:#x} rdi={:#x} rsi={:#x} rdx={:#x} parent={}",
        name, pid, entry, bootstrap.rdi, bootstrap.rsi, bootstrap.rdx, bootstrap.parent_pid
    );
    Ok(pid)
}

/// Patch bootstrap registers before the process's first userspace run.
pub fn set_bootstrap_regs(pid: u32, rdi: u64, rsi: u64, rdx: u64) {
    let idx = idx_of(pid);
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx] };
    if p.pid == pid {
        p.regs.rdi = rdi;
        p.regs.rsi = rsi;
        p.regs.rdx = rdx;
    }
}

pub fn claim_process_on_cpu(pid: u32, cpu_id: u32) {
    let idx = idx_of(pid);
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx] };
    if p.pid == pid {
        p.current_cpu = Some(cpu_id);
    }
}

/// Reap one zombie child of `parent`. Returns `(pid, exit_code)` or None.
pub fn reap_one_zombie(parent: u32) -> Option<(u32, i32)> {
    let _g = PTABLE_LOCK.lock();
    unsafe {
        for slot in PTABLE.iter_mut() {
            if slot.pid != 0
                && slot.parent_pid == parent
                && matches!(slot.state, ProcState::Zombie(_))
            {
                let pid = slot.pid;
                let code = slot.exit_code;
                *slot = Process::empty();
                return Some((pid, code));
            }
        }
    }
    None
}

/// Reap every zombie whose `parent_pid` matches `parent`. Returns count reaped.
pub fn reap_zombie_children(parent: u32) -> u32 {
    let mut reaped = 0u32;
    let _g = PTABLE_LOCK.lock();
    unsafe {
        for slot in PTABLE.iter_mut() {
            if slot.pid != 0
                && slot.parent_pid == parent
                && matches!(slot.state, ProcState::Zombie(_))
            {
                log::info!("[Process] init reaped pid={} code={}", slot.pid, slot.exit_code);
                *slot = Process::empty();
                reaped += 1;
            }
        }
    }
    reaped
}

/// Terminate a process.  May be called from the process itself (`sys_exit`)
/// or from another CPU (`kill`).
pub fn exit(pid: u32, code: i32) {
    let idx = idx_of(pid);
    let _g = PTABLE_LOCK.lock();
    // SAFETY: lock held.
    let p = unsafe { &mut PTABLE[idx] };
    if p.pid != pid || p.state == ProcState::Dead { return; }

    // Free the user page tables only for standalone processes — threads share
    // the parent's PML4 and must never free it. Instead, reclaim the thread's
    // private user stack and TLS page.
    if !p.is_thread {
        paging::free_user_pml4(p.pml4_phys);
    } else {
        // Only reclaim thread-private memory in the demand-pageable anon window.
        // Threads SHARE the parent's PML4, so a fs_base/stack that the Dart VM placed
        // in the main user stack (0x7FFF_xxxx) or a library image must NOT be unmapped
        // here — doing so clobbers pages other threads (incl. pid 1) are still using,
        // and those regions have no backing pager to re-fault them.
        let is_anon = |va: u64| va >= 0x3_0000_0000 && va < 0x100_0000_0000;
        if p.user_stack_base != 0 && p.user_stack_size != 0 && is_anon(p.user_stack_base) {
            paging::unmap_user_range(p.pml4_phys, p.user_stack_base, p.user_stack_size);
            dl::recycle_anon_va(pid, p.user_stack_base, p.user_stack_size);
        }
        if p.fs_base != 0 && is_anon(p.fs_base) {
            paging::unmap_user_range(p.pml4_phys, p.fs_base, 4096);
            dl::recycle_anon_va(pid, p.fs_base, 4096);
        }
    }
    free_syscall_stack(p.syscall_stack_base);
    p.syscall_stack_base = core::ptr::null_mut();
    p.syscall_stack_top = 0;

    p.state     = ProcState::Zombie(code);
    p.exit_code = code;
    p.current_cpu = None;
    log::info!("[Process] pid={} exited with code {}", pid, code);
}

/// Send SIGKILL to a process (forceful immediate exit).
pub fn kill(pid: u32) -> Result<(), &'static str> {
    let idx = idx_of(pid);
    {
        let _g = PTABLE_LOCK.lock();
        let p = unsafe { &PTABLE[idx] };
        if p.pid != pid || p.state == ProcState::Dead {
            return Err("no such process");
        }
    }
    exit(pid, -9); // SIGKILL = -9
    Ok(())
}

/// Set currently active userspace PID (called by exec/scheduler glue).
pub fn set_current_pid(pid: u32) {
    crate::arch::smp::this_cpu().current_pid.store(pid, Ordering::Release);
}

/// Get currently active userspace PID.
pub fn current_pid() -> u32 {
    crate::arch::smp::this_cpu().current_pid.load(Ordering::Acquire)
}

/// Wait for `pid` to become zombie, then reap it and return exit code.
///
/// Returns:
/// - `Ok(code)` if reaped
/// - `Err("not exited")` if still running
/// - `Err("no such process")` if pid is invalid/dead
pub fn waitpid(pid: u32) -> Result<i32, &'static str> {
    let idx = idx_of(pid);
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx] };
    if p.pid != pid || p.state == ProcState::Dead {
        return Err("no such process");
    }
    match p.state {
        ProcState::Zombie(code) => {
            // Reap the slot.
            *p = Process::empty();
            Ok(code)
        }
        _ => Err("not exited"),
    }
}

/// Look up a live process by PID.  Returns a copy of the saved registers.
pub fn get_regs(pid: u32) -> Option<UserRegs> {
    let idx = idx_of(pid);
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &PTABLE[idx] };
    if p.pid == pid && p.state == ProcState::Running {
        Some(p.regs)
    } else {
        None
    }
}

/// Save updated register state for a process (called from syscall / IRQ return).
pub fn save_regs(pid: u32, regs: UserRegs) {
    let idx = idx_of(pid);
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx] };
    if p.pid == pid { p.regs = regs; }
}

/// Save current CPU xstate image into process slot.
///
/// NOTE: intentionally does NOT check `p.state == Running`.  A blocking
/// syscall sets the thread's state to `Blocked` *before* calling this
/// function (to close a lost-wake race window), so the `Running` check
/// would silently skip the save and leave the buffer stale.  At the
/// call sites the CPU is always executing on behalf of `pid`, so saving
/// the live XMM/YMM/MXCSR to that slot is always correct.
pub fn save_xstate(pid: u32) {
    let idx = idx_of(pid);
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx] };
    if p.pid != pid {
        return;
    }
    let ptr = p.xstate.0.as_mut_ptr();
    unsafe { crate::arch::cpu::save_xstate_to(ptr) };
}

/// Restore CPU xstate image from process slot.
pub fn restore_xstate(pid: u32) {
    let idx = idx_of(pid);
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &PTABLE[idx] };
    if p.pid != pid || p.state != ProcState::Running {
        return;
    }
    let ptr = p.xstate.0.as_ptr();
    unsafe { crate::arch::cpu::restore_xstate_from(ptr) };
}

/// Safely attempt to claim the given PID for execution on `my_cpu`.
/// Returns `true` if the process is Running and either unclaimed or already assigned to `my_cpu`.
pub fn try_claim_cpu_for(pid: u32, my_cpu: u32) -> bool {
    let idx = idx_of(pid);
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx] };
    if p.pid == pid && p.state == ProcState::Running {
        if p.current_cpu.is_none() || p.current_cpu == Some(my_cpu) {
            p.current_cpu = Some(my_cpu);
            return true;
        }
    }
    false
}

/// Try-lock variant of `try_claim_cpu_for` to avoid ISR deadlocks.
pub fn try_claim_cpu_for_try(pid: u32, my_cpu: u32) -> bool {
    if let Some(_g) = PTABLE_LOCK.try_lock() {
        let idx = idx_of(pid);
        let p = unsafe { &mut PTABLE[idx] };
        if p.pid == pid && p.state == ProcState::Running {
            if p.current_cpu.is_none() || p.current_cpu == Some(my_cpu) {
                p.current_cpu = Some(my_cpu);
                return true;
            }
        }
        false
    } else {
        false
    }
}


/// Round-robin pick of next Running *user* thread that belongs to the same
/// address space as `current` (i.e. shares its `parent_pid` group), excluding
/// `current` itself. Returns `None` if no sibling thread is runnable.
///
/// Used to break user-space spin loops where the leader thread (e.g. pid 1)
/// is busy-calling pthread primitives while its worker threads (Dart UI /
/// Raster / IO workers) are technically Running but never get CPU time.
pub fn next_runnable_sibling_thread_locked(current: u32, my_cpu: u32) -> Option<u32> {
    // Determine the address-space group leader for `current`.
    let group: u32 = {
        let c = unsafe { &PTABLE[idx_of(current)] };
        if c.pid != current { return None; }
        get_group_leader_locked(current)
    };

    let start = idx_of(current.wrapping_add(1));
    for off in 0..MAX_PROCS {
        let idx = (start + off) % MAX_PROCS;
        let p = unsafe { &PTABLE[idx] };
        if p.pid == 0 || p.pid == current { continue; }
        if p.state != ProcState::Running { continue; }
        if !(p.current_cpu.is_none() || p.current_cpu == Some(my_cpu)) { continue; }
        // Member of the same address-space group?
        let p_group = get_group_leader_locked(p.pid);
        if p_group != group { continue; }
        // Skip the leader itself when picking a sibling.
        if p.pid == group { continue; }
        return Some(p.pid);
    }
    None
}

pub fn next_runnable_sibling_thread(current: u32) -> Option<u32> {
    let my_cpu = crate::arch::smp::this_cpu().cpu_id;
    let _g = PTABLE_LOCK.lock();
    if let Some(sib) = next_runnable_sibling_thread_locked(current, my_cpu) {
        let p = unsafe { &mut PTABLE[idx_of(sib)] };
        if p.current_cpu.is_none() || p.current_cpu == Some(my_cpu) {
            p.current_cpu = Some(my_cpu);
            return Some(sib);
        }
    }
    None
}

/// Collect every pid that belongs to the same address-space group as
/// `current` (including `current` itself and the group leader). Returns
/// an empty Vec if `current` is unknown.
///
/// Used by the cond_var bridge to wake all sibling-thread waiters when a
/// pthread_cond_broadcast on the leader finds zero waiters on the cond's
/// own futex address (Flutter engine's cond addresses don't match any of
/// the kernel's hardcoded futex target addresses).
pub fn sibling_pids(current: u32) -> alloc::vec::Vec<u32> {
    let _g = PTABLE_LOCK.lock();
    let mut out = alloc::vec::Vec::new();
    let group: u32 = {
        let c = unsafe { &PTABLE[idx_of(current)] };
        if c.pid != current { return out; }
        get_group_leader_locked(current)
    };
    for idx in 0..MAX_PROCS {
        let p = unsafe { &PTABLE[idx] };
        if p.pid == 0 { continue; }
        let p_group = get_group_leader_locked(p.pid);
        if p_group == group {
            out.push(p.pid);
        }
    }
    out
}

pub fn next_runnable_pid_locked(current: u32, my_cpu: u32) -> Option<u32> {
    let focus = crate::wm::focus_pid();
    let input_target = if focus != 0 { focus } else { 1 };
    if input_target != 0 && crate::wm::input_pending_for(input_target) > 0 {
        if current != input_target {
            let target = unsafe { &mut PTABLE[idx_of(input_target)] };
            if target.pid == input_target && target.state == ProcState::Running {
                if target.current_cpu.is_none() || target.current_cpu == Some(my_cpu) {
                    target.current_cpu = Some(my_cpu);
                    return Some(input_target);
                }
            }
        }
    }

    if current != 1 && crate::wm::embedder_baton_due() {
        let embedder = unsafe { &mut PTABLE[idx_of(1)] };
        if embedder.pid == 1 && embedder.state == ProcState::Running {
            if embedder.current_cpu.is_none() || embedder.current_cpu == Some(my_cpu) {
                embedder.current_cpu = Some(my_cpu);
                return Some(1);
            }
        }
    }

    let mut start = idx_of(current.wrapping_add(1));
    if current == 0 {
        start = 0;
    }

    // FOREGROUND-EXCLUSIVE SCHEDULING. When an app is foreground (focus is not the
    // shell pid 1), run ONLY that app's thread group and leave the backgrounded
    // shell suspended. Two concurrent heavy Flutter VMs overwhelm the cooperative
    // scheduler's context save/restore — a backgrounded host's thread gets switched
    // in with a corrupted resume RIP and #GPs. Serialising to one VM group at a time
    // keeps the (reliable) single-host path. focus=1 means the shell is foreground
    // and fg_group=1 covers the whole system as before (no behavioural change until
    // an app is launched). Idle (pid 0) and the launcher are never starved because
    // input/baton priority above already handled the focus pid.
    let fg = crate::wm::focus_pid();
    let fg_group = if fg > 1 { get_group_leader_locked(fg) } else { 1 };
    let exclusive = fg_group > 1;

    let mut res = None;
    for off in 0..MAX_PROCS {
        let idx = (start + off) % MAX_PROCS;
        let p = unsafe { &mut PTABLE[idx] };
        if p.pid != 0 && p.state == ProcState::Running {
            if exclusive && get_group_leader_locked(p.pid) != fg_group {
                continue; // not in the foreground app's group — skip while it runs
            }
            if p.current_cpu.is_none() || p.current_cpu == Some(my_cpu) {
                if current == 0 || p.pid != current {
                    p.current_cpu = Some(my_cpu);
                    res = Some(p.pid);
                    break;
                }
            }
        }
    }

    if res.is_none() && current != 0 {
        let c = unsafe { &mut PTABLE[idx_of(current)] };
        if c.pid == current && c.state == ProcState::Running
            && (!exclusive || get_group_leader_locked(current) == fg_group)
        {
            if c.current_cpu.is_none() || c.current_cpu == Some(my_cpu) {
                c.current_cpu = Some(my_cpu);
                res = Some(current);
            }
        }
    }

    res
}

pub fn next_runnable_pid(current: u32) -> Option<u32> {
    if PENDING_INIT_PID.load(Ordering::Acquire) != 0 {
        return None;
    }
    let my_cpu = crate::arch::smp::this_cpu().cpu_id;
    let _g = PTABLE_LOCK.lock();
    next_runnable_pid_locked(current, my_cpu)
}

/// Snapshot of user execution context used by timer preemption switching.
#[derive(Clone, Copy)]
pub struct UserContext {
    pub pid: u32,
    pub pml4_phys: u64,
    pub regs: UserRegs,
    pub syscall_stack_top: u64,
}

pub fn get_user_context(pid: u32) -> Option<UserContext> {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &PTABLE[idx_of(pid)] };
    if p.pid != pid || p.state != ProcState::Running {
        return None;
    }
    Some(UserContext {
        pid,
        pml4_phys: p.pml4_phys,
        regs: p.regs,
        syscall_stack_top: p.syscall_stack_top,
    })
}

pub fn current_syscall_stack_top() -> Option<u64> {
    let pid = current_pid();
    if pid == 0 {
        return None;
    }
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &PTABLE[idx_of(pid)] };
    if p.pid == pid && p.state == ProcState::Running {
        Some(p.syscall_stack_top)
    } else {
        None
    }
}

/// Count live processes.
pub fn count() -> usize {
    let _g = PTABLE_LOCK.lock();
    unsafe { PTABLE.iter().filter(|p| p.state != ProcState::Dead).count() }
}

// ── Thread creation ───────────────────────────────────────────────────────────

/// Spawn a new thread in `parent_pid`'s address space.
///
/// The thread shares the parent's PML4 (same address space) but gets its own:
/// * user stack allocated via [`dl::mmap_anon`]
/// * kernel syscall stack
/// * XSAVE buffer
/// * process-table slot (the scheduler treats it identically to a process)
///
/// Returns the thread ID (a PID-space value), or an error string.
pub fn spawn_thread(
    parent_pid: u32,
    entry_fn: u64,
    arg: u64,
    stack_size: usize,
) -> Result<u32, &'static str> {
    let (parent_pml4_phys, owning_pid) = {
        let _g = PTABLE_LOCK.lock();
        let p = unsafe { &PTABLE[idx_of(parent_pid)] };
        if p.pid != parent_pid || p.state == ProcState::Dead {
            log::error!("[spawn_thread] parent not found pid={}", parent_pid);
            return Err("parent not found");
        }
        (p.pml4_phys, get_group_leader_locked(parent_pid))
    };

    let tid = match alloc_pid() {
        Some(t) => t,
        None => {
            log::error!("[spawn_thread] alloc_pid failed (process table full)");
            return Err("process table full");
        }
    };
    let (sys_stack_base, sys_stack_top) = match alloc_syscall_stack() {
        Some(p) => p,
        None => {
            let (used, free, total) = crate::mm::heap::stats();
            log::error!("[spawn_thread] alloc_syscall_stack failed (heap OOM) used={} free={} total={}", used, free, total);
            return Err("OOM: syscall stack");
        }
    };

    // Allocate the thread's user stack inside the parent's address space.
    let stack_pages = (stack_size + 4095) / 4096;
    let stack_va = dl::mmap_anon(parent_pid, parent_pml4_phys, 0, stack_pages, 0x3 /* RW */);
    if stack_va == u64::MAX {
        log::error!("[spawn_thread] mmap_anon failed pages={} (frame OOM or VA OOM)", stack_pages);
        free_syscall_stack(sys_stack_base);
        return Err("OOM: thread stack");
    }
    log::warn!("[spawn_thread] alloc stack_va={:#x} pages={}", stack_va, stack_pages);
    let stack_top = stack_va + stack_size as u64;

    // Seed the bottom-most stack slot with the address of the internal
    // "thread return" trampoline (posix_trampolines::THREAD_RETURN_TRAMPOLINE_VA).
    // If the worker function returns normally instead of calling pthread_exit,
    // its `ret` will pop this address and land in the trampoline, which does
    // `mov rdi, rax; mov eax, 0x35B; syscall; ud2` so RAX (the function's
    // return value) becomes the thread exit code. Without this, RDI would be
    // an uninitialized register and the kernel would receive a garbage exit
    // code, which then propagates to pthread_join callers as a poisoned value
    // (typically a stack pointer that downstream code feeds into write(),
    // fwrite(), etc., triggering bogus EFAULTs and SIGABRT).
    //
    // The parent's PML4 is active here (we are in the parent's syscall
    // context), and `stack_va..stack_top` was just mmap'd RW into it, so
    // we can write to the user VA directly.
    unsafe {
        core::ptr::write_volatile(
            (stack_top - 8) as *mut u64,
            crate::process::posix_trampolines::thread_return_trampoline_va(),
        );
    }

    let mut regs = UserRegs::default();
    regs.rip    = entry_fn;
    regs.rdi    = arg;         // first argument via SysV ABI
    regs.rsp    = stack_top - 8; // RSP%16==8 at entry, per SysV ABI; the word
                                 // at [RSP] is the thread-return trampoline.
    regs.rflags = 0x0202;      // IF=1, reserved=1

    // Allocate a 4 KiB TLS/TCB page so the new thread has a valid FS base.
    // x86_64 musl/glibc TCB head layout: fs:0 must point at the TCB itself
    // (self-pointer). Without this, pthread_self() reads garbage and
    // start_routine bails immediately via pthread_exit.
    // The rest of the page is zeroed by mmap_anon, satisfying the common
    // fs:[+offset] reads used by setname/getspecific in early thread setup.
    let tls_va = dl::mmap_anon(parent_pid, parent_pml4_phys, 0, 1, 0x3 /* RW */);
    if tls_va == u64::MAX {
        log::error!("[spawn_thread] mmap_anon TLS failed");
        free_syscall_stack(sys_stack_base);
        return Err("OOM: thread TLS");
    }
    log::warn!("[spawn_thread] tid={} tls_va={:#x} arg={:#x}", tid, tls_va, arg);
    unsafe { core::ptr::write_volatile(tls_va as *mut u64, tls_va); }

    let my_cpu = crate::arch::smp::this_cpu().cpu_id;
    {
        let _g = PTABLE_LOCK.lock();
        let p = unsafe { &mut PTABLE[idx_of(tid)] };
        p.pid                = tid;
        p.pml4_phys          = parent_pml4_phys; // shared with parent
        p.regs               = regs;
        p.state              = ProcState::Running;
        p.exit_code          = 0;
        p.syscall_stack_base = sys_stack_base;
        p.syscall_stack_top  = sys_stack_top;
        p.xstate             = XStateBuf::default();
        p.is_thread          = true;
        p.parent_pid         = owning_pid;
        p.fs_base            = tls_va;
        // Record user stack bounds so pthread_attr_getstack can return them.
        p.user_stack_base    = stack_va;
        p.user_stack_size    = stack_size as u64;
        p.current_cpu        = None;
    }

    log::info!("[Process] Thread tid={} spawned in pid={} entry={:#x} tls={:#x}",
        tid, owning_pid, entry_fn, tls_va);
    Ok(tid)
}

/// Create a thread via `clone(2)` semantics.
///
/// Unlike [`spawn_thread`] the child's RIP is the syscall return address (i.e.
/// execution continues right after the `syscall` instruction in both parent
/// and child).  The child's initial RAX is 0 (clone returns 0 in child);
/// the parent's syscall return will carry the child TID.
///
/// `child_rsp` is the stack pointer the child should start with (as passed to
/// Linux clone as the `stack` argument).
pub fn clone_thread(
    parent_pid: u32,
    child_rip: u64,
    child_rsp: u64,
) -> Result<u32, &'static str> {
    // Save parent's registers from CPU scratch before reading them from PTABLE.
    save_full_user_gprs(parent_pid);

    let (parent_pml4_phys, owning_pid) = {
        let _g = PTABLE_LOCK.lock();
        let p = unsafe { &PTABLE[idx_of(parent_pid)] };
        if p.pid != parent_pid || p.state == ProcState::Dead {
            return Err("parent not found");
        }
        (p.pml4_phys, get_group_leader_locked(parent_pid))
    };

    let tid = alloc_pid().ok_or("process table full")?;
    let (sys_stack_base, sys_stack_top) = alloc_syscall_stack().ok_or("OOM: syscall stack")?;

    // Copy parent register snapshot so the child inherits callee-saved regs.
    let mut child_regs = {
        let _g = PTABLE_LOCK.lock();
        let p = unsafe { &PTABLE[idx_of(parent_pid)] };
        p.regs
    };
    // Diverge from parent: child starts at syscall return address with RAX=0.
    child_regs.rip    = child_rip;
    child_regs.rsp    = child_rsp;
    child_regs.rax    = 0; // clone(2): child receives 0
    child_regs.rflags = 0x0202;

    {
        let _g = PTABLE_LOCK.lock();
        let p = unsafe { &mut PTABLE[idx_of(tid)] };
        p.pid                = tid;
        p.pml4_phys          = parent_pml4_phys;
        p.regs               = child_regs;
        p.state              = ProcState::Running;
        p.exit_code          = 0;
        p.syscall_stack_base = sys_stack_base;
        p.syscall_stack_top  = sys_stack_top;
        p.xstate             = XStateBuf::default();
        p.is_thread          = true;
        p.parent_pid         = owning_pid;
        p.fs_base            = 0;
        p.current_cpu        = None;
        p.cpu_ticks          = 0;
        p.slice_left         = 10;
        p.pending_sigs       = 0;
        p.sig_mask           = 0;
        p.sig_handlers       = [0u64; 32];
        p.errno_to_deliver   = 0;
        p.preempted_by_timer = false;
        p.user_stack_base    = 0;
        p.user_stack_size    = 0;
    }

    log::info!("[Process] clone_thread tid={} in pid={} rip={:#x}", tid, owning_pid, child_rip);
    Ok(tid)
}

// ── User-space launch via SYSRET ──────────────────────────────────────────────

/// Schedule a kernel task that will SYSRET into the user process `pid`.
///
/// Must be called exactly once after [`spawn`] succeeds, before
/// `crate::cortex::run()` enables interrupts and the scheduler starts
/// dispatching.
pub fn schedule_user_launch(pid: u32) {
    PENDING_INIT_PID.store(pid, Ordering::Release);
    crate::sched::spawn_kernel_task("user-init", user_launch_task);
    log::info!("[Process] user-init kernel task registered for pid={}", pid);
}

pub fn enter_user_by_pid_noreturn(pid: u32) -> ! {
    let my_cpu = crate::arch::smp::this_cpu().cpu_id;
    let old_pid = current_pid();

    let (pml4_phys, rip, rsp, rflags, rax, rdi, rsi, rdx,
         rbx, rbp, r8, r9, r10, r11, r12, r13, r14, r15, rcx,
         syscall_stack_top, fs_base, errno_to_deliver, preempted_by_timer) = {
        let _g = PTABLE_LOCK.lock();
        if old_pid != 0 && old_pid != pid {
            let old_p = unsafe { &mut PTABLE[idx_of(old_pid)] };
            if old_p.pid == old_pid && old_p.current_cpu == Some(my_cpu) {
                old_p.current_cpu = None;
            }
        }
        let p = unsafe { &mut PTABLE[idx_of(pid)] };
        assert_eq!(p.pid, pid, "[Process] enter_user_by_pid_noreturn: stale PID");
        if let Some(other_cpu) = p.current_cpu {
            if other_cpu != my_cpu {
                panic!(
                    "[Process] enter_user_by_pid_noreturn: PID {} is already running on CPU {}, but CPU {} is trying to enter it!",
                    pid, other_cpu, my_cpu
                );
            }
        }
        p.current_cpu = Some(my_cpu);
        let e = p.errno_to_deliver;
        p.errno_to_deliver = 0; // consume it
        let pbt = p.preempted_by_timer;
        p.preempted_by_timer = false; // consume the flag
        (p.pml4_phys, p.regs.rip, p.regs.rsp, p.regs.rflags, p.regs.rax,
         p.regs.rdi, p.regs.rsi, p.regs.rdx,
         p.regs.rbx, p.regs.rbp, p.regs.r8, p.regs.r9, p.regs.r10,
         p.regs.r11, p.regs.r12, p.regs.r13, p.regs.r14, p.regs.r15,
         p.regs.rcx,
         p.syscall_stack_top, p.fs_base, e, pbt)
    };

    crate::process::set_current_pid(pid);
    crate::arch::syscall::set_active_stack_top(syscall_stack_top);
    crate::process::restore_xstate(pid);
    // Restore this thread's TLS pointer.  Always write the MSR (even with
    // 0): pthreads share an address space, and the FS_BASE MSR is per-CPU.
    // If we left the previous thread's FS_BASE in the MSR the new thread
    // would inherit its TLS and (a) trip Flutter's "Wakeable can only be
    // set once" check and (b) suppress the syscall-entry FS bootstrap (which
    // only fires when get_fs_base()==0). Writing 0 here forces bootstrap on
    // the new thread's first syscall.
    crate::arch::cpu::set_fs_base(fs_base);

    // Switch to the user address space.
    crate::arch::memory::write_cr3(pml4_phys);

    // Deliver pending errno (e.g. EINTR=4 from sys_epoll_wait_real) NOW,
    // after the CR3 switch so we write into the resuming thread's own SYSDATA
    // page.  This avoids the race where an intermediate thread switch
    // overwrites the shared errno slot before the target thread reads it.
    if errno_to_deliver != 0 {
        unsafe {
            core::ptr::write_volatile(
                crate::process::posix_trampolines::SD_ERRNO as *mut u32,
                errno_to_deliver,
            );
        }
    }

    // Assemble the full user register state for the arch entry hook. The
    // arch backend owns the actual ring-3 transition asm (IRETQ vs SYSRETQ on
    // x86_64); the shared code above owns all the PTABLE/CR3/errno logic.
    let enter_regs = crate::arch::EnterUserRegs {
        rip, rsp, rflags,
        rax, rbx, rcx, rdx, rsi, rdi, rbp,
        r8, r9, r10, r11, r12, r13, r14, r15,
    };

    if preempted_by_timer {
        // ── IRET path ────────────────────────────────────────────────────────
        // This thread was timer-preempted at an arbitrary user instruction, so
        // we MUST restore every register (including RCX, R11, and the exact
        // RFLAGS) before returning.  SYSRETQ is wrong here because it would
        // set RIP=RCX and RFLAGS=R11, corrupting those registers and losing
        // the real FLAGS state (e.g. CF/ZF from a cmp instruction).
        unsafe { crate::arch::enter_user_iret(&enter_regs) }
    }

    // ── SYSRET path (syscall-yield context) ──────────────────────────────────
    // Use SYSRET to enter user mode. This is the native x86_64 user-entry path
    // and mirrors how SYSCALL returns to user code. The full user GPR set is
    // restored so a thread that yielded via a syscall (e.g. futex_wait) sees
    // exactly the register state it had at the SYSCALL instant, with only
    // RAX changed to the syscall return value. This is required because the
    // SysV ABI mandates that callee-saved regs (rbx, rbp, r12–r15) survive
    // a function call, and the userspace SYSCALL trampoline is such a call.
    log::trace!("[enter_user] about to SYSRET. rip={:#x} rsp={:#x} rflags={:#x} pml4_phys={:#x}", rip, rsp, rflags, pml4_phys);
    unsafe { crate::arch::enter_user_sysret(&enter_regs) }
}

pub fn enter_user_by_pid_noreturn_try(pid: u32) -> bool {
    let context = {
        let mut g = match PTABLE_LOCK.try_lock() {
            Some(guard) => guard,
            None => return false,
        };
        let p = unsafe { &mut PTABLE[idx_of(pid)] };
        if p.pid != pid {
            return false;
        }
        let e = p.errno_to_deliver;
        p.errno_to_deliver = 0; // consume it
        let pbt = p.preempted_by_timer;
        p.preempted_by_timer = false; // consume the flag
        let context = (p.pml4_phys, p.regs.rip, p.regs.rsp, p.regs.rflags, p.regs.rax,
         p.regs.rdi, p.regs.rsi, p.regs.rdx,
         p.regs.rbx, p.regs.rbp, p.regs.r8, p.regs.r9, p.regs.r10,
         p.regs.r11, p.regs.r12, p.regs.r13, p.regs.r14, p.regs.r15,
         p.regs.rcx,
         p.syscall_stack_top, p.fs_base, e, pbt);
        drop(g);
        context
    };

    let (pml4_phys, rip, rsp, rflags, rax, rdi, rsi, rdx,
         rbx, rbp, r8, r9, r10, r11, r12, r13, r14, r15, rcx,
         syscall_stack_top, fs_base, errno_to_deliver, preempted_by_timer) = context;

    crate::process::set_current_pid(pid);
    if pid == 8 || pid == 9 {
        static ENTER_USER_89_LOG: AtomicU32 = AtomicU32::new(0);
        let n = ENTER_USER_89_LOG.fetch_add(1, Ordering::Relaxed);
        if n < 32 {
            log::warn!(
                "[enter-user-89-try] #{} pid={} rip={:#x} rsp={:#x} fs={:#x} preempted={} errno={}",
                n,
                pid,
                rip,
                rsp,
                fs_base,
                preempted_by_timer,
                errno_to_deliver
            );
        }
    }
    crate::arch::syscall::set_active_stack_top(syscall_stack_top);
    crate::process::restore_xstate(pid);
    crate::arch::cpu::set_fs_base(fs_base);

    // Switch to the user address space.
    crate::arch::memory::write_cr3(pml4_phys);

    if errno_to_deliver != 0 {
        unsafe {
            core::ptr::write_volatile(
                crate::process::posix_trampolines::SD_ERRNO as *mut u32,
                errno_to_deliver,
            );
        }
    }

    let enter_regs = crate::arch::EnterUserRegs {
        rip, rsp, rflags,
        rax, rbx, rcx, rdx, rsi, rdi, rbp,
        r8, r9, r10, r11, r12, r13, r14, r15,
    };

    if preempted_by_timer {
        unsafe { crate::arch::enter_user_iret(&enter_regs) }
    }

    unsafe { crate::arch::enter_user_sysret(&enter_regs) }
}

/// Kernel-task entry point: loads the user PML4 and SYSRETs to ring-3.
///
/// This function never returns — it ends with `sysretq` which transfers
/// control to the user process.  The scheduler task slot for "user-init"
/// will remain in the `Running` state and will never be re-scheduled (the
/// round-robin only selects `Ready` tasks), which is the correct behaviour
/// for a single-user-process kernel.
fn user_launch_task() {
    let pid = PENDING_INIT_PID.load(Ordering::Acquire);
    log::info!("[Process] launching userspace pid={} via SYSRET", pid);
    // Disable interrupts so the APIC timer cannot preempt between
    // mark_current_zombie() and sysretq.  SYSRET restores RFLAGS from R11
    // (0x0202 = IF=1), so the user process runs with interrupts enabled.
    crate::arch::interrupts_disable();
    // Mark this kernel task as Zombie before we SYSRET.  The SYSRET transfers
    // execution to ring-3 permanently; this task slot must never be resumed as
    // a kernel task again.  Marking it Zombie prevents the scheduler from
    // saving a corrupt interrupt-stack RSP into our kernel_sp on subsequent
    // preemptions, and prevents the round-robin from ever selecting it again.
    crate::sched::mark_current_zombie();
    let my_cpu = crate::arch::smp::this_cpu().cpu_id;
    {
        let _g = PTABLE_LOCK.lock();
        let p = unsafe { &mut PTABLE[idx_of(pid)] };
        if p.pid == pid {
            p.current_cpu = Some(my_cpu);
        }
    }
    PENDING_INIT_PID.store(0, Ordering::Release);
    enter_user_by_pid_noreturn(pid)
}

// ── Phase 47: parent-blocking helpers ────────────────────────────────────────

/// Called from the APIC timer ISR when `cur_pid`'s user thread is being
/// preempted.  Saves the full register state in `cur_regs` (captured from the
/// hardware interrupt frame + hardware-pushed user RSP), marks the thread as
/// timer-preempted so `enter_user_by_pid_noreturn` uses IRETQ on its next
/// scheduled run, then finds the next runnable thread and returns its register
/// set so the ISR can rewrite the interrupt frame.
///
/// Returns `None` if there is no other runnable thread (no switch occurs).
pub fn timer_preempt_switch(cur_pid: u32, cur_regs: &UserRegs) -> Option<(u32, UserRegs, u64)> {
    let my_cpu = crate::arch::smp::this_cpu().cpu_id;
    let fs = crate::arch::cpu::get_fs_base();

    let _g = PTABLE_LOCK.lock();

    // 1. Find the next runnable thread (under lock)
    let next_pid = next_runnable_pid_locked(cur_pid, my_cpu)?;
    if next_pid == cur_pid {
        return None;
    }

    // 2. Save the current thread's context
    let p_cur = unsafe { &mut PTABLE[idx_of(cur_pid)] };
    if p_cur.pid == cur_pid && p_cur.state == ProcState::Running {
        p_cur.regs = *cur_regs;
        p_cur.regs.rflags |= 0x200; // ensure IF=1 on resume
        p_cur.preempted_by_timer = true;
        p_cur.fs_base = fs;
        p_cur.current_cpu = None; // Switch away from cur_pid
    }

    // Save XSTATE for cur_pid
    if p_cur.pid == cur_pid {
        let ptr = p_cur.xstate.0.as_mut_ptr();
        unsafe { crate::arch::cpu::save_xstate_to(ptr) };
    }

    // 3. Load the next thread's context
    let p_next = unsafe { &mut PTABLE[idx_of(next_pid)] };
    if p_next.pid != next_pid || p_next.state != ProcState::Running {
        p_cur.current_cpu = Some(my_cpu);
        return None;
    }

    p_next.current_cpu = Some(my_cpu);
    let next_regs = p_next.regs;

    // Restore XSTATE for next_pid
    let ptr_next = p_next.xstate.0.as_ptr();
    unsafe { crate::arch::cpu::restore_xstate_from(ptr_next) };

    // Update the per-CPU scheduler state
    crate::process::set_current_pid(next_pid);

    let stack_top = p_next.syscall_stack_top;
    let next_fs_base = p_next.fs_base;
    let pml4_phys = p_next.pml4_phys;

    drop(_g); // Release lock

    // Point the active syscall stack and restore FS base (outside lock)
    crate::arch::syscall::set_active_stack_top(stack_top);
    crate::arch::cpu::set_fs_base(next_fs_base);

    Some((next_pid, next_regs, pml4_phys))
}

/// Try-lock variant of `timer_preempt_switch` to avoid ISR deadlocks.
pub fn timer_preempt_switch_try(cur_pid: u32, cur_regs: &UserRegs) -> Option<(u32, UserRegs, u64)> {
    let my_cpu = crate::arch::smp::this_cpu().cpu_id;
    let fs = crate::arch::cpu::get_fs_base();

    if let Some(_g) = PTABLE_LOCK.try_lock() {
        // 1. Find the next runnable thread (under lock)
        let next_pid = next_runnable_pid_locked(cur_pid, my_cpu)?;
        if next_pid == cur_pid {
            return None;
        }

        // 2. Save the current thread's context
        let p_cur = unsafe { &mut PTABLE[idx_of(cur_pid)] };
        if p_cur.pid == cur_pid && p_cur.state == ProcState::Running {
            p_cur.regs = *cur_regs;
            p_cur.regs.rflags |= 0x200; // ensure IF=1 on resume
            p_cur.preempted_by_timer = true;
            p_cur.fs_base = fs;
            p_cur.current_cpu = None; // Switch away from cur_pid
        }

        // Save XSTATE for cur_pid
        if p_cur.pid == cur_pid {
            let ptr = p_cur.xstate.0.as_mut_ptr();
            unsafe { crate::arch::cpu::save_xstate_to(ptr) };
        }

        // 3. Load the next thread's context
        let p_next = unsafe { &mut PTABLE[idx_of(next_pid)] };
        if p_next.pid != next_pid || p_next.state != ProcState::Running {
            p_cur.current_cpu = Some(my_cpu);
            return None;
        }

        p_next.current_cpu = Some(my_cpu);
        let next_regs = p_next.regs;

        // Restore XSTATE for next_pid
        let ptr_next = p_next.xstate.0.as_ptr();
        unsafe { crate::arch::cpu::restore_xstate_from(ptr_next) };

        // Update the per-CPU scheduler state
        crate::process::set_current_pid(next_pid);

        let stack_top = p_next.syscall_stack_top;
        let next_fs_base = p_next.fs_base;
        let pml4_phys = p_next.pml4_phys;

        drop(_g); // Release lock

        // Point the active syscall stack and restore FS base (outside lock)
        crate::arch::syscall::set_active_stack_top(stack_top);
        crate::arch::cpu::set_fs_base(next_fs_base);

        Some((next_pid, next_regs, pml4_phys))
    } else {
        None
    }
}

/// Save full user context at a SYSCALL boundary before cooperatively yielding
/// to another process. Marks the thread so timer preemption resumes via IRETQ.
pub fn save_cooperative_yield_context(pid: u32, rip: u64, rsp: u64) {
    save_full_user_gprs(pid);
    let fs = crate::arch::cpu::get_fs_base();
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx_of(pid)] };
    if p.pid != pid {
        return;
    }
    p.regs.rip = rip;
    p.regs.rsp = rsp;
    p.regs.rflags = 0x202;
    p.regs.rcx = rip;
    p.regs.r11 = 0x202;
    p.preempted_by_timer = true;
    p.fs_base = fs;
}

/// Save the user-space return context (RIP + RSP after syscall) into the
/// process's register file so `enter_user_by_pid_noreturn` resumes correctly.
pub fn save_return_context(pid: u32, rip: u64, rsp: u64) {
    let fs = crate::arch::cpu::get_fs_base();
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx_of(pid)] };
    if p.pid == pid {
        p.regs.rip    = rip;
        p.regs.rsp    = rsp;
        p.regs.rflags = 0x202; // IF=1, standard user RFLAGS
        // Saved at a SYSCALL boundary — SYSRETQ is correct for this thread.
        p.preempted_by_timer = false;
        p.fs_base = fs;
    }
}

/// Snapshot the full user GPR set (as captured at SYSCALL entry) into the
/// process's register file. Must be called from inside a syscall handler
/// before yielding the CPU to another process via
/// `enter_user_by_pid_noreturn`, otherwise the yielding thread will resume
/// with stale rbx/rbp/r12–r15/rdi/rsi/etc. and corrupt its C++ caller's
/// `this` pointer and locals.
pub fn save_full_user_gprs(pid: u32) {
    let snap = crate::arch::syscall::user_gprs();
    if pid == 2 {
        log::warn!(
            "[save_full_user_gprs] pid=2 rdi={:#x} rsi={:#x} rbx={:#x} rbp={:#x} r12={:#x} r13={:#x} r14={:#x} r15={:#x}",
            snap.rdi,
            snap.rsi,
            snap.rbx,
            snap.rbp,
            snap.r12,
            snap.r13,
            snap.r14,
            snap.r15
        );
    }
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx_of(pid)] };
    if p.pid != pid { return; }
    p.regs.rdi = snap.rdi;
    p.regs.rsi = snap.rsi;
    p.regs.rdx = snap.rdx;
    p.regs.r10 = snap.r10;
    p.regs.r8  = snap.r8;
    p.regs.r9  = snap.r9;
    p.regs.rbx = snap.rbx;
    p.regs.rbp = snap.rbp;
    p.regs.r12 = snap.r12;
    p.regs.r13 = snap.r13;
    p.regs.r14 = snap.r14;
    p.regs.r15 = snap.r15;
}

/// Set the `rax` return value that will be delivered when this process is
/// next entered via `enter_user_by_pid_noreturn`.
pub fn set_rax(pid: u32, val: u64) {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx_of(pid)] };
    if p.pid == pid { p.regs.rax = val; }
}

/// Schedule an errno value to be written into SD_ERRNO (SYSDATA_VA+32) for
/// `pid` immediately after the CR3 switch in `enter_user_by_pid_noreturn`,
/// before SYSRET.  This avoids the race where an intermediate thread switch
/// overwrites the shared errno slot before the target thread reads it.
pub fn set_errno_to_deliver(pid: u32, errno: u32) {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx_of(pid)] };
    if p.pid == pid { p.errno_to_deliver = errno; }
}

/// Read and clear the pending errno-to-deliver for `pid`.
/// Used by the timer ISR to deliver errno when resuming a cooperative-yielded
/// thread via IRETQ (bypassing `enter_user_by_pid_noreturn`).
pub fn take_errno_to_deliver(pid: u32) -> u32 {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx_of(pid)] };
    if p.pid == pid {
        let e = p.errno_to_deliver;
        p.errno_to_deliver = 0;
        e
    } else {
        0
    }
}

/// Read the saved rax for a pid (for diagnostics).
pub fn get_saved_rax(pid: u32) -> u64 {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &PTABLE[idx_of(pid)] };
    if p.pid == pid { p.regs.rax } else { 0 }
}

/// Saved user RIP/RSP for a blocked/yielded thread (from `save_return_context`).
pub fn get_saved_rip_rsp(pid: u32) -> Option<(u64, u64)> {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &PTABLE[idx_of(pid)] };
    if p.pid == pid {
        Some((p.regs.rip, p.regs.rsp))
    } else {
        None
    }
}

/// Force a process into the `Blocked` or `Running` state.
pub fn set_state(pid: u32, state: ProcState) {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx_of(pid)] };
    if p.pid == pid {
        let old_state = p.state;
        p.state = state;
        // Note: CPU occupancy is kept as Some(cpu) even when blocked to prevent
        // another CPU core from concurrently scheduling this thread while its
        // kernel execution context is still bound/sleeping on the original CPU.
        // It is cleared only when the CPU switches to a different userspace PID.
        if state == ProcState::Running && old_state != ProcState::Running {
            drop(_g);
            crate::arch::smp::broadcast_resched_ipi();
            return;
        }
    }
}

/// Try-lock variant of `set_state` to avoid ISR deadlocks.
pub fn set_state_try(pid: u32, state: ProcState) -> bool {
    if let Some(_g) = PTABLE_LOCK.try_lock() {
        let p = unsafe { &mut PTABLE[idx_of(pid)] };
        if p.pid == pid {
            let old_state = p.state;
            p.state = state;
            if state == ProcState::Running && old_state != ProcState::Running {
                drop(_g);
                crate::arch::smp::broadcast_resched_ipi();
            }
            true
        } else {
            false
        }
    } else {
        false
    }
}

/// Dump a compact scheduler snapshot for the main Flutter bring-up threads.
/// Used by syscall diagnostics to explain deadlock conditions.
pub fn debug_dump_core_threads() {
    let _g = PTABLE_LOCK.lock();
    for &pid in &[1u32, 5u32, 6u32, 8u32] {
        let p = unsafe { &PTABLE[idx_of(pid)] };
        if p.pid != pid {
            log::warn!("[sched-snapshot] pid={} <absent>", pid);
            continue;
        }
        let state = match p.state {
            ProcState::Dead => "Dead",
            ProcState::Running => "Running",
            ProcState::Blocked => "Blocked",
            ProcState::Zombie(_) => "Zombie",
        };
        log::warn!(
            "[sched-snapshot] pid={} state={} rip={:#x} rsp={:#x} rax={:#x} cpu_ticks={} slice_left={}",
            pid,
            state,
            p.regs.rip,
            p.regs.rsp,
            p.regs.rax,
            p.cpu_ticks,
            p.slice_left,
        );
    }
}

/// True if `pid` is currently `Blocked` (i.e. genuinely waiting in
/// waitpid / exec_wait / cond_wait / similar).  Used by `sys_exit` to
/// decide whether the parent should be woken with the child's exit
/// code — clobbering a running parent's RAX would corrupt unrelated
/// in-flight syscalls (e.g. pthread_cond_wait expecting 0 on signal).
pub fn is_blocked(pid: u32) -> bool {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &PTABLE[idx_of(pid)] };
    p.pid == pid && p.state == ProcState::Blocked
}

/// Try-lock variant of `is_blocked` to avoid ISR deadlocks.
pub fn is_blocked_try(pid: u32) -> bool {
    if let Some(_g) = PTABLE_LOCK.try_lock() {
        let p = unsafe { &PTABLE[idx_of(pid)] };
        p.pid == pid && p.state == ProcState::Blocked
    } else {
        false
    }
}

/// Record which process is waiting for `child_pid` to exit.
/// Overwrites whatever `parent_pid` was set during spawn.
pub fn set_child_parent(child_pid: u32, parent_pid: u32) {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx_of(child_pid)] };
    if p.pid == child_pid { p.parent_pid = parent_pid; }
}

/// Return the PID of the process waiting for `pid` to exit, if any.
pub fn get_parent_of(pid: u32) -> Option<u32> {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &PTABLE[idx_of(pid)] };
    if p.pid == pid && p.parent_pid != 0 {
        Some(p.parent_pid)
    } else {
        None
    }
}

/// Reap a zombie process (free the slot without waiting for parent).
/// Used by sys_exit when the parent is unblocked inline.
pub fn reap_zombie(pid: u32) {
    let idx = idx_of(pid);
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx] };
    if p.pid == pid {
        if let ProcState::Zombie(_) = p.state {
            *p = Process::empty();
        }
    }
}

// ── Phase 53: time-slice + CPU accounting ─────────────────────────────────────

/// Called from the APIC timer ISR to account one tick and check if the
/// current process has exhausted its quantum.  Returns `true` if the process
/// should be preempted now.
pub fn account_tick(pid: u32) -> bool {
    let idx = idx_of(pid);
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx] };
    if p.pid != pid || p.state != ProcState::Running { return false; }
    p.cpu_ticks += 1;
    if p.slice_left > 0 { p.slice_left -= 1; }
    let preempt = p.slice_left == 0;
    if preempt { p.slice_left = 10; } // reset quantum
    preempt
}

/// Try-lock variant of `account_tick` to avoid ISR deadlocks.
pub fn account_tick_try(pid: u32) -> Option<bool> {
    if let Some(_g) = PTABLE_LOCK.try_lock() {
        let idx = idx_of(pid);
        let p = unsafe { &mut PTABLE[idx] };
        if p.pid != pid || p.state != ProcState::Running { return Some(false); }
        p.cpu_ticks += 1;
        if p.slice_left > 0 { p.slice_left -= 1; }
        let preempt = p.slice_left == 0;
        if preempt { p.slice_left = 10; } // reset quantum
        Some(preempt)
    } else {
        None
    }
}

/// Return cumulative CPU ticks for a process.
pub fn get_cpu_ticks(pid: u32) -> u64 {
    let idx = idx_of(pid);
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &PTABLE[idx] };
    if p.pid == pid { p.cpu_ticks } else { 0 }
}

// ── Phase 55: signal delivery ─────────────────────────────────────────────────

/// Raise signal `sig` (1–31) on process `pid`.
pub fn raise_signal(pid: u32, sig: u8) {
    if sig == 0 || sig > 31 { return; }
    let idx = idx_of(pid);
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx] };
    if p.pid == pid && p.state != ProcState::Dead {
        p.pending_sigs |= 1 << (sig - 1);
    }
}

/// Check for and deliver the lowest-numbered pending, unmasked signal.
/// Returns `Some((sig, handler_va))` — caller must set up the signal frame.
/// handler_va == 0 → default action (for SIGKILL/SIGTERM: terminate).
/// handler_va == 1 → SIG_IGN.
pub fn dequeue_signal(pid: u32) -> Option<(u8, u64)> {
    let idx = idx_of(pid);
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx] };
    if p.pid != pid { return None; }
    let deliverable = p.pending_sigs & !p.sig_mask;
    if deliverable == 0 { return None; }
    let bit = deliverable.trailing_zeros() as u8;
    p.pending_sigs &= !(1u32 << bit);
    let sig = bit + 1;
    let handler = p.sig_handlers[bit as usize];
    Some((sig, handler))
}

/// Set the handler VA for signal `sig`.  0 = default, 1 = ignore.
pub fn set_signal_handler(pid: u32, sig: u8, handler_va: u64) {
    if sig == 0 || sig > 31 { return; }
    let idx = idx_of(pid);
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx] };
    if p.pid == pid { p.sig_handlers[(sig - 1) as usize] = handler_va; }
}

/// Set signal mask for a process.
pub fn set_signal_mask(pid: u32, mask: u32) {
    let idx = idx_of(pid);
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx] };
    if p.pid == pid { p.sig_mask = mask; }
}

// ── Phase 54: fork helpers ─────────────────────────────────────────────────────

/// Clone the current process (fork semantic). Returns child PID or error.
/// The child starts with identical register state; `rax` will be set to 0
/// in the child by the caller (sys_fork).
pub fn fork_current() -> Result<u32, &'static str> {
    let parent_pid = current_pid();
    if parent_pid == 0 { return Err("fork from kernel"); }

    // Save parent's registers from CPU scratch before reading them from PTABLE.
    save_full_user_gprs(parent_pid);

    let child_pid = alloc_pid().ok_or("too many processes")?;
    let child_slot = idx_of(child_pid);

    // Read parent's state.
    let (parent_pml4, parent_regs, parent_stack_top) = {
        let _g = PTABLE_LOCK.lock();
        let p = unsafe { &PTABLE[idx_of(parent_pid)] };
        (p.pml4_phys, p.regs, p.syscall_stack_top)
    };

    // Allocate a new PML4 for the child and deep-copy user pages.
    let child_pml4 = clone_address_space(parent_pml4)?;

    // Inherit the parent's VA bump cursors so the child allocator
    // starts past the already-mapped region, not from ANON_VA_BASE.
    crate::process::dl::clone_as_slot(parent_pml4, child_pml4);

    // Allocate a new syscall stack for child.
    let (child_stack_base, child_stack_top) = alloc_syscall_stack()
        .ok_or("no memory for child stack")?;

    let _g = PTABLE_LOCK.lock();
    let child = unsafe { &mut PTABLE[child_slot] };
    child.pid               = child_pid;
    child.pml4_phys         = child_pml4;
    child.regs              = parent_regs;
    child.regs.rax          = 0;   // child gets return value 0
    child.state             = ProcState::Running;
    child.exit_code         = 0;
    child.syscall_stack_base = child_stack_base;
    child.syscall_stack_top  = child_stack_top;
    child.is_thread          = false;
    child.parent_pid         = parent_pid;
    child.cpu_ticks          = 0;
    child.slice_left         = 10;
    child.pending_sigs       = 0;
    child.sig_mask           = 0;
    child.sig_handlers       = [0u64; 32];

    Ok(child_pid)
}

/// Deep-copy an address space (simplified: maps all present user pages as new
/// writable pages with copied content — true COW comes later).
fn clone_address_space(src_pml4_phys: u64) -> Result<u64, &'static str> {
    let hhdm = crate::mm::frame_allocator::hhdm_offset();
    let new_pml4_phys = crate::mm::frame_allocator::alloc_frame().ok_or("no frames for fork")?;
    let new_pml4_va = new_pml4_phys + hhdm;
    // Zero the new PML4.
    unsafe { core::ptr::write_bytes(new_pml4_va as *mut u8, 0, 4096); }

    // Walk the source PML4 entries [0..255] (user half) and copy page tables.
    let src_pml4_va = src_pml4_phys + hhdm;
    unsafe {
        let src = src_pml4_va as *const u64;
        let dst = new_pml4_va as *const u64 as *mut u64;
        for i in 0..256usize {
            let entry = src.add(i).read_volatile();
            if entry & 1 == 0 { continue; }
            // Deep-copy this PML3.
            let child_entry = deep_copy_pt(entry, hhdm, 3)?;
            dst.add(i).write_volatile(child_entry);
        }
        // Copy kernel mappings (PML4[256..511]) by reference — shared, not owned.
        let kernel_src = src_pml4_va as *const [u64; 512];
        let kernel_dst = new_pml4_va as *mut [u64; 512];
        for i in 256..512usize {
            (*kernel_dst)[i] = (*kernel_src)[i];
        }
    }
    Ok(new_pml4_phys)
}

/// Recursively copy a page-table level.  `level` = 3 (PML3) down to 1 (PT).
/// For level 1, allocates new physical frames and copies page content.
unsafe fn deep_copy_pt(src_entry: u64, hhdm: u64, level: u8) -> Result<u64, &'static str> {
    let src_phys = src_entry & 0x000f_ffff_ffff_f000u64;
    let new_phys = crate::mm::frame_allocator::alloc_frame().ok_or("no frames (fork pt)")?;
    let src_va   = src_phys + hhdm;
    let dst_va   = new_phys + hhdm;
    core::ptr::write_bytes(dst_va as *mut u8, 0, 4096);

    let src_tbl = src_va as *const u64;
    let dst_tbl = dst_va as *mut u64;

    for i in 0..512usize {
        let child = src_tbl.add(i).read_volatile();
        if child & 1 == 0 { continue; }
        let new_child = if level == 1 {
            // Leaf: allocate a fresh page and copy content.
            let page_phys = crate::mm::frame_allocator::alloc_frame().ok_or("no frames (fork page)")?;
            let src_page_va = (child & 0x000f_ffff_ffff_f000u64) + hhdm;
            let dst_page_va = page_phys + hhdm;
            core::ptr::copy_nonoverlapping(src_page_va as *const u8, dst_page_va as *mut u8, 4096);
            // Same flags but pointing to the new physical frame.
            (child & !0x000f_ffff_ffff_f000u64) | page_phys
        } else {
            deep_copy_pt(child, hhdm, level - 1)?
        };
        dst_tbl.add(i).write_volatile(new_child);
    }
    // Preserve flags from the source entry, but point to new table.
    let flags = src_entry & 0xFFF;
    Ok(new_phys | flags)
}

/// Find a thread ID (PID) by its FS base (TCB pointer).
pub fn find_tid_by_fs_base(fs_base: u64) -> Option<u32> {
    let _g = PTABLE_LOCK.lock();
    for p in unsafe { PTABLE.iter() } {
        if p.state != ProcState::Dead && p.fs_base == fs_base {
            return Some(p.pid);
        }
    }
    None
}

/// Get the FS base (TLS pointer) for a thread.
pub fn get_fs_base(pid: u32) -> u64 {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &PTABLE[idx_of(pid)] };
    if p.pid == pid && p.state != ProcState::Dead {
        p.fs_base
    } else {
        0
    }
}

/// Dump a list of all active user threads/processes.
pub fn debug_dump_processes() {
    if PTABLE_LOCK.try_lock().is_some() {
        {
            let _g = PTABLE_LOCK.lock();
            log::info!("=== Process Table Dump ===");
            for p in unsafe { PTABLE.iter() } {
                if p.state != ProcState::Dead {
                    let state_str = match p.state {
                        ProcState::Dead => "Dead",
                        ProcState::Running => "Running",
                        ProcState::Blocked => "Blocked",
                        ProcState::Zombie(_code) => "Zombie",
                    };
                    log::info!(
                        "  pid={} state={} is_thread={} parent_pid={} rip={:#x} rsp={:#x} rax={:#x} rdi={:#x} rsi={:#x} fs_base={:#x} ticks={}",
                        p.pid, state_str, p.is_thread, p.parent_pid, p.regs.rip, p.regs.rsp, p.regs.rax, p.regs.rdi, p.regs.rsi, p.fs_base, p.cpu_ticks
                    );
                }
            }
        }
        crate::syscall::debug_dump_sync_states();
        log::info!("==========================");
    }
}
