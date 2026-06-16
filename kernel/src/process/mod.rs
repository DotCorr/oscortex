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
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
    /// Capability set governing access to privileged syscalls. Stored per-PCB
    /// (not derived from PID): the system shell (HOST_MODE_SHELL) gets the full
    /// set, launched apps get none, and threads/forks inherit their creator's.
    pub caps: crate::security::Capabilities,
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
    /// SMP/preemption safety (Redox pattern): false while this thread is inside a
    /// critical region that must not be preempted or migrated to another core
    /// (e.g. holding a userspace mutex/futex). The timer-preempt path and the
    /// cross-core scheduler skip a non-preemptable thread instead of switching it
    /// away — preventing the mid-critical-section corruption that broke preempted
    /// Dart engine threads. Defaults true. See [[redox-blueprint]].
    pub is_preemptable: bool,
    /// aarch64 only: full EL0 trap-frame snapshot (x0..x30, SP_EL0, ELR, SPSR,
    /// ESR) captured when this thread is timer-preempted at an arbitrary user
    /// instruction. The shared `UserRegs` carries only the x86-named subset, so
    /// ARM scratch registers (x6/x7, x11–x18, x24–x28, x30) would otherwise be
    /// lost across a preemptive switch. The ARM timer ISR saves the live
    /// `vectors::TrapFrame` here and restores it on resume, giving full-fidelity
    /// preemption while the shared scheduler still owns the switch *decision*.
    /// Layout matches `arch::aarch64::vectors::TrapFrame` (102 × u64):
    /// [0..35]=GPRs/sysregs/pad, [36..100)=v0..v31, 100=FPSR, 101=FPCR. The FP
    /// region doubles as the cooperative-yield FP slot (see `arch_fp_valid`).
    #[cfg(target_arch = "aarch64")]
    pub arch_trapframe: [u64; 102],
    /// aarch64 only: true once `arch_trapframe` holds a valid timer-preempt
    /// snapshot to restore (vs. a fresh/syscall-entered thread).
    #[cfg(target_arch = "aarch64")]
    pub arch_frame_valid: bool,
    /// aarch64 only: true once the FP region of `arch_trapframe` (words 36..102)
    /// holds this thread's FP/SIMD file captured at SVC entry by the vector stub
    /// (before kernel NEON ran), so a cooperative resume can restore it.
    #[cfg(target_arch = "aarch64")]
    pub arch_fp_valid: bool,
    /// aarch64 only: the user link register (x30) captured at the last SVC
    /// boundary. AArch64 keeps return addresses in LR, not on the stack, so a
    /// thread that yields mid-syscall must have x30 restored on resume — without
    /// it the cooperative `build_image` re-entry leaves x30=0 and the thread's
    /// next `ret` branches to address 0.
    #[cfg(target_arch = "aarch64")]
    pub user_lr: u64,
    /// aarch64 only: the user PC (ELR) and SP (SP_EL0) captured at SVC ENTRY,
    /// while the per-CPU snapshot is fresh (the eager entry capture). A blocking
    /// syscall's `save_return_context` rewinds/uses THESE rather than the live
    /// per-CPU scratch, which a timer-preempted sibling's SVC can clobber before
    /// the yield decision — otherwise the thread resumes on the wrong SP and its
    /// `ldp x29,x30,[sp,#..]; ret` reads a stale/zero frame record → `ret` to 0.
    #[cfg(target_arch = "aarch64")]
    pub entry_user_rip: u64,
    #[cfg(target_arch = "aarch64")]
    pub entry_user_rsp: u64,
    /// aarch64 only: callee-saved x24–x28 captured at SVC entry. The shared
    /// x86-named UserRegs has no slots for these (x86 only has r8–r15), so the
    /// cooperative/re-exec resume (`build_image`) would otherwise lose them
    /// (→ 0) and the Dart VM, which keeps live pointers there, faults near-null.
    #[cfg(target_arch = "aarch64")]
    pub user_x24: u64,
    #[cfg(target_arch = "aarch64")]
    pub user_x25: u64,
    #[cfg(target_arch = "aarch64")]
    pub user_x26: u64,
    #[cfg(target_arch = "aarch64")]
    pub user_x27: u64,
    #[cfg(target_arch = "aarch64")]
    pub user_x28: u64,
    /// aarch64 only: true if this thread's saved context is a syscall RETURN
    /// (resume PC is *after* the `svc`, so the syscall's return value must land in
    /// x0) rather than a RE-EXEC (resume PC rewound to the `svc`, so x0 must hold
    /// the original arg0 and x8 the syscall nr). On x86 the return value and the
    /// re-exec nr share one register (rax) so no distinction is needed; on aarch64
    /// they are x0 vs x8, so a RETURN-mode resume must override x0 ← rax. Without
    /// this a cooperatively-resumed `pthread_cond_broadcast` (return 0) delivered
    /// x0 = arg0 = the cond pointer, which Dart read as a nonzero pthread error
    /// and aborted (synchronization_posix.cc:164).
    #[cfg(target_arch = "aarch64")]
    pub aarch64_ret_in_x0: bool,
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
            caps:      crate::security::NO_CAPS,
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
            is_preemptable: true,
            #[cfg(target_arch = "aarch64")]
            arch_trapframe: [0u64; 102],
            #[cfg(target_arch = "aarch64")]
            arch_frame_valid: false,
            #[cfg(target_arch = "aarch64")]
            arch_fp_valid: false,
            #[cfg(target_arch = "aarch64")]
            user_lr: 0,
            #[cfg(target_arch = "aarch64")]
            entry_user_rip: 0,
            #[cfg(target_arch = "aarch64")]
            entry_user_rsp: 0,
            #[cfg(target_arch = "aarch64")]
            user_x24: 0,
            #[cfg(target_arch = "aarch64")]
            user_x25: 0,
            #[cfg(target_arch = "aarch64")]
            user_x26: 0,
            #[cfg(target_arch = "aarch64")]
            user_x27: 0,
            #[cfg(target_arch = "aarch64")]
            user_x28: 0,
            #[cfg(target_arch = "aarch64")]
            aarch64_ret_in_x0: false,
        }
    }
}

/// aarch64: save a full EL0 trap-frame snapshot into `pid`'s arch slot, taken
/// when the generic-timer ISR preempts the thread. Restored verbatim by
/// `arch_take_trapframe` so ARM scratch registers survive the switch.
///
/// Called from the timer IRQ handler, so it uses `try_lock` and never blocks —
/// blocking on a contended PTABLE_LOCK from inside an ISR would deadlock. Returns
/// false if the lock was busy (the caller then skips the switch this tick).
#[cfg(target_arch = "aarch64")]
pub fn arch_store_trapframe(pid: u32, frame: &[u64; 102]) -> bool {
    if let Some(_g) = PTABLE_LOCK.try_lock() {
        let p = unsafe { &mut PTABLE[idx_of(pid)] };
        if p.pid == pid {
            p.arch_trapframe = *frame;
            p.arch_frame_valid = true;
            // The full frame includes FP; mark it valid too so a cooperative
            // resume of this timer-preempted thread restores FP from it.
            p.arch_fp_valid = true;
        }
        true
    } else {
        false
    }
}

/// aarch64: fetch `pid`'s saved trap-frame snapshot (and whether it is valid).
/// The valid flag is consumed (cleared) so a subsequent syscall-entry re-entry
/// rebuilds the frame from `regs` instead of replaying a stale snapshot. Uses
/// `try_lock` for the same ISR-safety reason as `arch_store_trapframe`.
/// ISR-safe (try_lock) variant: returns None on lock contention. Used from the
/// timer ISR. The COOPERATIVE resume path must use the BLOCKING variant below,
/// because a try_lock miss there silently drops a timer-preempted thread to the
/// lossy build_image rebuild (which zeroes the live caller-saved x6/x7/x9..x18
/// the thread held at its arbitrary preemption point → nondeterministic Dart
/// corruption: "Invalid UTF8" / EC=0x24 crash).
#[cfg(target_arch = "aarch64")]
pub fn arch_take_trapframe_try(pid: u32) -> Option<[u64; 102]> {
    let _g = PTABLE_LOCK.try_lock()?;
    arch_take_trapframe_locked(pid)
}

/// Blocking variant for syscall/cooperative context (where blocking on
/// PTABLE_LOCK is safe). NEVER misses a timer-preempted thread's full frame.
#[cfg(target_arch = "aarch64")]
pub fn arch_take_trapframe(pid: u32) -> Option<[u64; 102]> {
    let _g = PTABLE_LOCK.lock();
    arch_take_trapframe_locked(pid)
}

#[cfg(target_arch = "aarch64")]
fn arch_take_trapframe_locked(pid: u32) -> Option<[u64; 102]> {
    let p = unsafe { &mut PTABLE[idx_of(pid)] };
    if p.pid == pid && p.arch_frame_valid {
        p.arch_frame_valid = false;
        // FP rides this frame; the resume path consumes both.
        p.arch_fp_valid = false;
        Some(p.arch_trapframe)
    } else {
        None
    }
}

/// aarch64: stash the FP/SIMD file from a live trap frame (captured at SVC entry
/// by the vector stub, BEFORE kernel NEON ran) into `pid`'s arch slot, so a
/// cooperative resume can restore it. Writes ONLY the FP region (words 36..102),
/// leaving the GPR words to the cooperative GPR snapshot.
#[cfg(target_arch = "aarch64")]
pub fn aarch64_store_fp_from_frame(pid: u32, f: &crate::arch::aarch64::vectors::TrapFrame) {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx_of(pid)] };
    if p.pid != pid { return; }
    for i in 0..32 {
        p.arch_trapframe[36 + i * 2] = f.v[i][0];
        p.arch_trapframe[36 + i * 2 + 1] = f.v[i][1];
    }
    p.arch_trapframe[100] = f.fpsr;
    p.arch_trapframe[101] = f.fpcr;
    p.arch_fp_valid = true;
}

/// aarch64: take `pid`'s saved FP image (64 v-words + FPSR + FPCR) for a
/// cooperative resume, or None if no FP was captured (fresh thread). Does NOT
/// clear arch_frame_valid (the GPR snapshot path owns that).
#[cfg(target_arch = "aarch64")]
pub fn aarch64_take_fp(pid: u32) -> Option<[u64; 66]> {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &PTABLE[idx_of(pid)] };
    if p.pid != pid || !p.arch_fp_valid { return None; }
    let mut out = [0u64; 66];
    out[..64].copy_from_slice(&p.arch_trapframe[36..100]);
    out[64] = p.arch_trapframe[100];
    out[65] = p.arch_trapframe[101];
    Some(out)
}

/// aarch64: read the callee-saved x24–x28 captured for `pid` (used to populate
/// EnterUserRegs so the cooperative/re-exec resume restores them).
#[cfg(target_arch = "aarch64")]
fn callee_saved_x24_x28(pid: u32) -> (u64, u64, u64, u64, u64) {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &PTABLE[idx_of(pid)] };
    if p.pid == pid {
        (p.user_x24, p.user_x25, p.user_x26, p.user_x27, p.user_x28)
    } else {
        (0, 0, 0, 0, 0)
    }
}

/// aarch64, ISR-safe: read the callee-saved x24–x28 *and* the saved link
/// register (x30) for `pid`. Used by the timer-IRQ handler to make the lossy
/// `userregs_to_trapframe` fallback restore the registers the shared x86-named
/// `UserRegs` cannot carry (x24–x28, x30). Without this, a thread whose last
/// suspension was a cooperative syscall yield (so it has no full ARM snapshot)
/// inherits the *preempted* thread's x24–x28/x30 when the timer ISR resumes it
/// via the fallback — e.g. a leaked screen dimension lands in x30 and the next
/// `ret` branches to it. Uses `try_lock`; returns None on contention (caller
/// then leaves the fallback as-is for this tick).
#[cfg(target_arch = "aarch64")]
pub fn aarch64_resume_extras_try(pid: u32) -> Option<(u64, u64, u64, u64, u64, u64)> {
    let _g = PTABLE_LOCK.try_lock()?;
    let p = unsafe { &PTABLE[idx_of(pid)] };
    if p.pid == pid {
        Some((p.user_x24, p.user_x25, p.user_x26, p.user_x27, p.user_x28, p.user_lr))
    } else {
        None
    }
}

/// aarch64, ISR-safe: true if `pid`'s saved context is a syscall RETURN (so the
/// timer-ISR fallback must deliver the syscall result in x0, not the stale
/// arg0). Returns None on lock contention. See `aarch64_ret_in_x0`.
#[cfg(target_arch = "aarch64")]
pub fn aarch64_ret_in_x0_try(pid: u32) -> Option<bool> {
    let _g = PTABLE_LOCK.try_lock()?;
    let p = unsafe { &PTABLE[idx_of(pid)] };
    if p.pid == pid { Some(p.aarch64_ret_in_x0) } else { None }
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

/// CPU index used to key PTABLE_LOCK_RECURSION / the holder atomic. Always a
/// valid in-range index. We use the computed `current_cpu_id()` (which on
/// aarch64 short-circuits to 0 for single-core) rather than reading the
/// `PerCpuData.cpu_id` *field*, and hard-clamp to the recursion-array bound so
/// the timer ISR can never index out of bounds and panic the kernel even if a
/// per-CPU struct read is momentarily inconsistent.
#[inline(always)]
fn ptable_cpu_idx() -> u32 {
    let id = crate::arch::smp::current_cpu_id();
    if (id as usize) < crate::arch::smp::MAX_CPUS { id } else { 0 }
}

impl PTableLock {
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(()),
            holder: AtomicU32::new(0xFFFF_FFFF),
        }
    }

    pub fn lock(&self) -> PTableGuard {
        let my_cpu = ptable_cpu_idx();
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
            preempt_disable_cpu(my_cpu); // non-preemptable while holding PTABLE_LOCK
            PTableGuard { is_outer: true }
        }
    }

    pub fn try_lock(&self) -> Option<PTableGuard> {
        let my_cpu = ptable_cpu_idx();
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
                preempt_disable_cpu(my_cpu); // non-preemptable while holding PTABLE_LOCK
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
            let my_cpu = ptable_cpu_idx();
            unsafe {
                PTABLE_LOCK_RECURSION[my_cpu as usize] = 0;
                PTABLE_LOCK.holder.store(0xFFFF_FFFF, Ordering::Release);
                PTABLE_LOCK_GUARD = None;
            }
            preempt_enable_cpu(my_cpu); // preemptable again once PTABLE_LOCK is released
        } else {
            let my_cpu = ptable_cpu_idx();
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

// ── SMP M1: per-CPU preempt-disable + non-preemptable bail ───────────────────
//
// Redox-blueprint pattern #2 ("the key to safe preemption"): never switch AWAY
// from a CPU that is in a critical section. Here the critical section is "this
// CPU holds PTABLE_LOCK" — auto-tracked by incrementing this CPU's preempt depth
// on the OUTER PTABLE_LOCK acquire and decrementing it on the outer release. The
// timer-preempt path checks `preempt_disabled()` and bails when set.
//
// Why this matters even single-core (x86): the timer ISR's `timer_preempt_switch_
// try` does a *recursive* PTABLE_LOCK `try_lock`, which SUCCEEDS when the
// interrupted thread already holds the lock — it could then switch threads while
// the lock is logically held, leaving `holder` pinned to this CPU and wedging
// every later PTABLE_LOCK user. Bailing when preempt-disabled closes that hole.
// One slot per CPU; only the owning CPU touches its slot (+ its own ISR reads it).
pub static PREEMPT_DEPTH: [AtomicU32; crate::arch::smp::MAX_CPUS] =
    [const { AtomicU32::new(0) }; crate::arch::smp::MAX_CPUS];

#[inline(always)]
fn preempt_disable_cpu(cpu: u32) {
    if (cpu as usize) < crate::arch::smp::MAX_CPUS {
        PREEMPT_DEPTH[cpu as usize].fetch_add(1, Ordering::AcqRel);
    }
}
#[inline(always)]
fn preempt_enable_cpu(cpu: u32) {
    if (cpu as usize) < crate::arch::smp::MAX_CPUS {
        // Saturating: never wrap below zero on an unbalanced release.
        let slot = &PREEMPT_DEPTH[cpu as usize];
        let mut cur = slot.load(Ordering::Acquire);
        while cur > 0 {
            match slot.compare_exchange_weak(cur, cur - 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => break,
                Err(v) => cur = v,
            }
        }
    }
}

/// True if THIS CPU is currently non-preemptable (in a PTABLE_LOCK critical
/// section, or another future preempt-disabled region). The timer-preempt path
/// must not switch away when this is set.
#[inline]
pub fn preempt_disabled() -> bool {
    let c = ptable_cpu_idx() as usize;
    c < crate::arch::smp::MAX_CPUS && PREEMPT_DEPTH[c].load(Ordering::Acquire) > 0
}

/// Global context-switch serialization lock (Redox-blueprint #2). Reserved for the
/// M4 two-layer `switch_to`; declared now so the foundation is in one place.
#[allow(dead_code)]
pub static CONTEXT_SWITCH_LOCK: AtomicBool = AtomicBool::new(false);

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

    // Map POSIX trampoline + sysdata pages so libc/glibc symbols the Flutter
    // engine imports resolve to in-process syscall stubs. encode_stub() emits
    // native machine code per arch (x86 on x86_64, AArch64 on aarch64), so this
    // is required on BOTH arches once a dynamically-linked host (the Flutter
    // embedder) runs — the engine's DT_INIT_ARRAY jumps into these stubs.
    posix_trampolines::map_system_pages(pml4_phys)?;

    let idx = idx_of(pid);
    let mut regs = UserRegs::default();
    regs.rip    = entry;
    // AArch64 requires a 16-byte-aligned SP (enforced by SCTLR_EL1.SA on real
    // hardware); x86 just wants a guard word. USER_STACK_TOP is page-aligned, so
    // use it directly on aarch64 and keep the -8 guard word on x86.
    #[cfg(target_arch = "aarch64")]
    { regs.rsp = USER_STACK_TOP & !0xF; }
    #[cfg(not(target_arch = "aarch64"))]
    { regs.rsp = USER_STACK_TOP - 8; } // leave one guard word
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
        // Capabilities: the trusted system shell (HOST_MODE_SHELL) holds the
        // full set; everything else launched this way (HOST_MODE_APP) starts
        // with none and must be granted caps explicitly. This is what makes
        // privileged syscalls capability-gated rather than PID-gated.
        p.caps = if bootstrap.rdi == crate::app_registry::HOST_MODE_SHELL {
            crate::security::ALL_CAPS
        } else {
            // Apps get NET so they can use the userspace TCP/DNS socket API
            // (the abstraction layer). Everything else stays gated. The security
            // phase will replace this blanket grant with a per-app declared/
            // prompted NET permission rather than granting every app by default.
            crate::security::Capabilities::NET
        };
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
        // aarch64: start with no stale timer-preempt frame snapshot.
        #[cfg(target_arch = "aarch64")]
        {
            p.arch_frame_valid = false;
            p.arch_fp_valid = false;
        }
    }

    log::info!(
        "[Process] Spawned '{}' pid={} entry={:#x} rdi={:#x} rsi={:#x} rdx={:#x} parent={} caps={:?}",
        name, pid, entry, bootstrap.rdi, bootstrap.rsi, bootstrap.rdx, bootstrap.parent_pid,
        caps_of(pid)
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

    // Crash auto-recovery: tell the app registry which thread GROUP this exit
    // belongs to. Resolved while the lock is held; the (try-lock, ISR-safe)
    // notification itself runs after we release it.
    let leader = get_group_leader_locked(pid);
    drop(_g);
    crate::app_registry::note_thread_exit(leader, pid, code);
}

/// Forcefully terminate every still-live member of `leader`'s thread group
/// (including the leader itself). Used by crash recovery: when one thread of a
/// launched app faults, the survivors must be torn down before a fresh
/// instance is launched — two engine instances of the same app corrupt each
/// other under the cooperative scheduler. Returns the number killed.
///
/// Must be called from a normal (syscall) context, NOT an ISR: it takes the
/// PTABLE lock to collect victims, then calls `exit()` per victim.
pub fn kill_group(leader: u32) -> u32 {
    let mut victims = [0u32; MAX_PROCS];
    let mut n = 0usize;
    {
        let _g = PTABLE_LOCK.lock();
        for slot in unsafe { PTABLE.iter() } {
            if slot.pid == 0 {
                continue;
            }
            if matches!(slot.state, ProcState::Zombie(_) | ProcState::Dead) {
                continue;
            }
            if get_group_leader_locked(slot.pid) == leader && n < victims.len() {
                victims[n] = slot.pid;
                n += 1;
            }
        }
    }
    for &v in &victims[..n] {
        exit(v, -9);
    }
    n as u32
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

/// Mark a thread (non-)preemptable (Redox pattern). Set false while it holds a
/// userspace lock so the timer-preempt path + cross-core scheduler won't switch
/// it away / migrate it mid-critical-section. ISR/lock-free-safe via try_lock.
pub fn set_preemptable(pid: u32, value: bool) {
    if let Some(_g) = PTABLE_LOCK.try_lock() {
        let p = unsafe { &mut PTABLE[idx_of(pid)] };
        if p.pid == pid {
            p.is_preemptable = value;
        }
    }
}

/// Is `pid` currently preemptable? Defaults true for unknown/dead pids so callers
/// never wedge on a stale id.
pub fn is_preemptable(pid: u32) -> bool {
    if let Some(_g) = PTABLE_LOCK.try_lock() {
        let p = unsafe { &PTABLE[idx_of(pid)] };
        if p.pid == pid {
            return p.is_preemptable;
        }
    }
    true
}

/// Set currently active userspace PID (called by exec/scheduler glue).
pub fn set_current_pid(pid: u32) {
    crate::arch::smp::this_cpu().current_pid.store(pid, Ordering::Release);
}

/// Get currently active userspace PID.
pub fn current_pid() -> u32 {
    crate::arch::smp::this_cpu().current_pid.load(Ordering::Acquire)
}

/// The capability set held by `pid` (NO_CAPS if the pid is unknown/dead).
pub fn caps_of(pid: u32) -> crate::security::Capabilities {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &PTABLE[idx_of(pid)] };
    if p.pid == pid && p.state != ProcState::Dead {
        p.caps
    } else {
        crate::security::NO_CAPS
    }
}

/// Whether the CURRENTLY-RUNNING process holds all of `required`. The single
/// gate every privileged syscall calls — capability-based, not PID-based.
pub fn current_has_caps(required: crate::security::Capabilities) -> bool {
    crate::security::check(caps_of(current_pid()), required)
}

/// Physical address of the current process's top-level page table (the L1 / TTBR0
/// root on aarch64, the PML4 on x86). Used by diagnostics / kernel-side user-VA
/// translation. Returns 0 if there is no current process.
pub fn current_user_root() -> u64 {
    let pid = current_pid();
    if pid == 0 { return 0; }
    let idx = idx_of(pid);
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &PTABLE[idx] };
    if p.pid == pid { p.pml4_phys } else { 0 }
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
    // aarch64: FP/SIMD is captured in the vector stub at trap entry (TrapFrame.v)
    // and round-trips via arch_trapframe / the cooperative FP slot — NOT here.
    // This runs AFTER the syscall handler's NEON memcpy/memset, so saving the live
    // v-file now would store kernel garbage. No-op on aarch64.
    #[cfg(target_arch = "aarch64")]
    { let _ = pid; }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let idx = idx_of(pid);
        let _g = PTABLE_LOCK.lock();
        let p = unsafe { &mut PTABLE[idx] };
        if p.pid != pid {
            return;
        }
        let ptr = p.xstate.0.as_mut_ptr();
        unsafe { crate::arch::cpu::save_xstate_to(ptr) };
    }
}

/// Restore CPU xstate image from process slot.
pub fn restore_xstate(pid: u32) {
    // aarch64: FP rides the trap frame (see save_xstate). No-op here.
    #[cfg(target_arch = "aarch64")]
    { let _ = pid; }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let idx = idx_of(pid);
        let _g = PTABLE_LOCK.lock();
        let p = unsafe { &PTABLE[idx] };
        if p.pid != pid || p.state != ProcState::Running {
            return;
        }
        let ptr = p.xstate.0.as_ptr();
        unsafe { crate::arch::cpu::restore_xstate_from(ptr) };
    }
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

/// Compact debug string of pid/state/current_cpu for the first few PIDs, for
/// diagnosing why the timer-preempt scheduler can't find a runnable thread.
pub fn debug_runnable_states() -> alloc::string::String {
    use core::fmt::Write;
    let mut s = alloc::string::String::new();
    if let Some(_g) = PTABLE_LOCK.try_lock() {
        // Print the RAW slot contents for indices 1..=12 (the slot's own .pid
        // field, not the loop var) so a slot whose .pid != index reveals a
        // reused/cleared entry — the mechanism behind a worker thread that runs
        // yet never appears under its expected index.
        for idx in 1usize..=12 {
            let p = unsafe { &PTABLE[idx] };
            if p.pid == 0 { continue; }
            let st = match p.state {
                ProcState::Running => 'R',
                ProcState::Blocked => 'B',
                ProcState::Zombie(_) => 'Z',
                ProcState::Dead => 'D',
            };
            let cpu = match p.current_cpu { Some(c) => c as i32, None => -1 };
            // slot[idx]=pid:state c cpu
            let _ = write!(s, "s{}={}{}{} ", idx, p.pid, st, cpu);
        }
    } else {
        s.push_str("(locked)");
    }
    s
}

pub fn next_runnable_pid_locked(current: u32, my_cpu: u32) -> Option<u32> {
    // FOREGROUND-EXCLUSIVE SCHEDULING. When an app is foreground (focus is not the
    // shell pid 1), run ONLY that app's thread group and leave the backgrounded
    // shell suspended. Two concurrent heavy Flutter VMs overwhelm the cooperative
    // scheduler's context save/restore — a backgrounded host's thread gets switched
    // in with a corrupted callee-saved reg (rbx/r12=garbage `this`) and #GPs (e.g.
    // the app's dart EventHandler::Poll). Serialising to one VM group at a time keeps
    // the (reliable) single-host path. Computed FIRST so the input/baton priority
    // shortcuts below also honour it — otherwise a due shell (pid 1) baton would
    // schedule the shell engine alongside a foreground app, which is exactly the
    // two-VM concurrency that crashes the launched app.
    let fg = crate::wm::focus_pid();
    let fg_group = if fg > 1 { get_group_leader_locked(fg) } else { 1 };
    let mut exclusive = fg_group > 1;
    // If the foreground group's LEADER is dead (the app crashed), exclusivity
    // must lapse — otherwise the filter below skips every other process
    // (including the shell) and the whole system wedges on a dead group.
    // The shell then gets scheduled again and crash recovery (app_registry
    // drain in sys_wm_next_event) can refocus + relaunch.
    if exclusive {
        let leader = unsafe { &PTABLE[idx_of(fg_group)] };
        let leader_alive = leader.pid == fg_group
            && !matches!(leader.state, ProcState::Zombie(_) | ProcState::Dead);
        if !leader_alive {
            exclusive = false;
        }
    }

    let focus = fg;
    let input_target = if focus != 0 { focus } else { 1 };
    // Only honour the input shortcut when the target belongs to the foreground
    // group (or nothing is exclusive). Prevents jumping to a backgrounded host.
    let input_ok = !exclusive || get_group_leader_locked(input_target) == fg_group;
    if input_ok && input_target != 0 && crate::wm::input_pending_for(input_target) > 0 {
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

    // Shell (pid 1) baton shortcut — SUPPRESSED while an app is foreground, so the
    // shell engine cannot run concurrently with the launched app. Per-engine: only
    // the SHELL's own baton triggers this (the app's baton runs the app instead).
    if !exclusive && current != 1 && crate::wm::embedder_baton_due(1) {
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

/// Page-table root (PML4 on x86, TTBR0 on aarch64) of any LIVE process — including
/// Blocked ones (unlike `get_user_context`, which requires Running). Used by the
/// futex to compute a waiter's PHYSICAL futex address for SMP-correct wake scoping
/// (a Blocked futex waiter must still be locatable). Returns None for Dead slots.
pub fn pml4_phys_of(pid: u32) -> Option<u64> {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &PTABLE[idx_of(pid)] };
    if p.pid == pid && p.state != ProcState::Dead {
        Some(p.pml4_phys)
    } else {
        None
    }
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
/// aarch64: eagerly back a single anonymous user page with a zeroed frame and
/// map it, so a subsequent kernel write to that VA does not take a nested EL1
/// demand fault. No-op if the page is already mapped.
#[cfg(target_arch = "aarch64")]
fn eager_map_anon_page(pml4_phys: u64, va: u64) {
    let page_va = va & !0xFFF;
    if crate::mm::paging::translate_user_page(pml4_phys, page_va).is_some() {
        return;
    }
    if let Some(phys) = crate::mm::frame_allocator::alloc_frame() {
        let hhdm = (phys + crate::mm::frame_allocator::hhdm_offset()) as *mut u8;
        unsafe { core::ptr::write_bytes(hhdm, 0, 4096); }
        let _ = unsafe {
            crate::mm::paging::map_user_page_with_flags(pml4_phys, page_va, phys, true, true)
        };
    }
}

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
    // aarch64: pre-map the exact page we are about to write so the kernel write
    // does NOT take a nested EL1 demand-page fault. Every nested abort pushes a
    // ~288-byte TrapFrame onto the (single, shared) EL1 kernel stack and runs the
    // whole pager on it; pre-mapping the two pages spawn_thread writes removes
    // two nested fault chains per spawn and keeps the EL1 stack shallow.
    #[cfg(target_arch = "aarch64")]
    eager_map_anon_page(parent_pml4_phys, stack_top - 8);
    unsafe {
        core::ptr::write_volatile(
            (stack_top - 8) as *mut u64,
            crate::process::posix_trampolines::thread_return_trampoline_va(),
        );
    }

    let mut regs = UserRegs::default();
    regs.rip    = entry_fn;
    regs.rdi    = arg;         // first argument via SysV ABI
    // Thread-entry stack pointer. The two arches have OPPOSITE alignment rules:
    //   x86 SysV: at a function's first instruction RSP%16==8 (the CALL pushed an
    //     8-byte return address), and the word at [RSP] is the return target — so
    //     we seed `stack_top - 8` with the thread-return trampoline (above).
    //   AArch64: SP must be 16-BYTE ALIGNED at all times (SCTLR_EL1.SA enforces it
    //     on real hardware — HVF / bare metal / a Raspberry Pi), and the return
    //     address lives in x30/LR, not on the stack (seeded via p.user_lr below).
    //     `stack_top - 8` gave an 8-aligned SP that TCG tolerated but real silicon
    //     faults on (EC=0x26 SP-alignment) the instant the new thread touches its
    //     stack — which is why nothing rendered under HVF/UTM. Align down to 16.
    #[cfg(target_arch = "aarch64")]
    { regs.rsp = stack_top & !0xF; }
    #[cfg(not(target_arch = "aarch64"))]
    { regs.rsp = stack_top - 8; }
    regs.rflags = 0x0202;      // IF=1, reserved=1

    // ── Thread TLS block ──────────────────────────────────────────────────────
    // The engine calls a custom pthread_create that bypasses glibc's user-space
    // TLS setup, so the kernel builds the thread's TLS block itself. `tls_va` is
    // the value loaded into the thread's TLS-base register (FS_BASE / TPIDR_EL0).
    #[cfg(target_arch = "aarch64")]
    let tls_va = {
        // aarch64 = variant-I TLS: TPIDR_EL0 -> TCB (16 bytes: dtv, private); the
        // static TLS block (.tdata copy + .tbss zero) sits at TP+16, and a
        // __thread var at module offset A lives at TP+16+A — exactly where the
        // dl resolves its TLSDESC/TPREL relocations. A single zeroed page makes
        // the Dart VM's __thread vars read 0 → near-null deref in ThreadRegistry.
        // Build a real block initialised from the engine's captured PT_TLS image.
        const TLS_TCB_SIZE: u64 = 16;
        let tmpl = dl::get_tls_template(parent_pml4_phys);
        let memsz = tmpl.map_or(0, |t| t.memsz);
        let pages = ((TLS_TCB_SIZE as usize + memsz as usize) + 4095) / 4096;
        let va = dl::mmap_anon(parent_pid, parent_pml4_phys, 0, pages.max(1), 0x3 /* RW */);
        if va == u64::MAX {
            log::error!("[spawn_thread] mmap_anon TLS failed");
            free_syscall_stack(sys_stack_base);
            return Err("OOM: thread TLS");
        }
        for pg in 0..pages.max(1) {
            eager_map_anon_page(parent_pml4_phys, va + (pg * 4096) as u64);
        }
        unsafe {
            // TCB head at TP+0 (variant-I tcbhead_t: dtv, private). Static TLS
            // needs no DTV; zero both words.
            core::ptr::write_volatile(va as *mut u64, 0);
            core::ptr::write_volatile((va + 8) as *mut u64, 0);
            if let Some(t) = tmpl {
                let dst = (va + TLS_TCB_SIZE) as *mut u8;
                core::ptr::write_bytes(dst, 0, memsz as usize); // .tbss
                if t.filesz > 0 {
                    core::ptr::copy_nonoverlapping(
                        t.image_va as *const u8, dst, t.filesz as usize,
                    ); // .tdata initial image
                }
            }
        }
        log::warn!("[spawn_thread] tid={} tls_va={:#x} tls_memsz={:#x} arg={:#x}", tid, va, memsz, arg);
        va
    };
    #[cfg(not(target_arch = "aarch64"))]
    let tls_va = {
        // x86_64 = variant-II TLS (block before the TCB, __thread at negative
        // offsets from fs:0); a single page whose [0] self-points as the TCB head
        // is sufficient for the engine's thread use here.
        let va = dl::mmap_anon(parent_pid, parent_pml4_phys, 0, 1, 0x3 /* RW */);
        if va == u64::MAX {
            log::error!("[spawn_thread] mmap_anon TLS failed");
            free_syscall_stack(sys_stack_base);
            return Err("OOM: thread TLS");
        }
        unsafe { core::ptr::write_volatile(va as *mut u64, va); }
        log::warn!("[spawn_thread] tid={} tls_va={:#x} arg={:#x}", tid, va, arg);
        va
    };

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
        // Threads inherit their owning process's capabilities.
        p.caps               = unsafe { core::ptr::addr_of!(PTABLE[idx_of(owning_pid)].caps).read() };
        p.fs_base            = tls_va;
        // Record user stack bounds so pthread_attr_getstack can return them.
        p.user_stack_base    = stack_va;
        p.user_stack_size    = stack_size as u64;
        p.current_cpu        = None;
        // aarch64: the return address lives in the link register (x30), NOT on
        // the stack as on x86. A fresh thread is entered via build_image which
        // maps the saved LR from `user_lr`; seed it with the thread-return
        // trampoline so a worker start routine that RETURNS (instead of calling
        // pthread_exit) lands in the trampoline (which turns x0 into the thread
        // exit code) rather than branching to the leftover rflags value 0x0202.
        #[cfg(target_arch = "aarch64")]
        { p.user_lr = crate::process::posix_trampolines::thread_return_trampoline_va(); }
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
        // Threads inherit their owning process's capabilities.
        p.caps               = unsafe { core::ptr::addr_of!(PTABLE[idx_of(owning_pid)].caps).read() };
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

/// Clear the pending-init guard. `next_runnable_pid` (the wrapper) returns None
/// while `PENDING_INIT_PID != 0`, which suppresses scheduling until the very first
/// user process is launched. On x86 `user_launch_task` clears it as part of the
/// SYSRET launch; on aarch64 the boot enters the init process DIRECTLY via
/// `enter_user_by_pid_noreturn` (main.rs) and never runs `user_launch_task`, so
/// the guard stayed at its initial 1 forever — making `next_runnable_pid` always
/// return None. That is invisible during normal operation (cooperative yields and
/// the timer use `next_runnable_pid_locked`, which has no guard) but freezes
/// `sys_exit`: when any thread exits it calls the wrapper, gets None, and halts
/// forever even though siblings are runnable. The aarch64 launch path MUST call
/// this immediately before entering the init process.
pub fn mark_init_launched() {
    PENDING_INIT_PID.store(0, Ordering::Release);
}

pub fn enter_user_by_pid_noreturn(req_pid: u32) -> ! {
    let my_cpu = crate::arch::smp::this_cpu().cpu_id;
    let old_pid = current_pid();

    // ── SMP claim resolution ────────────────────────────────────────────────
    // Release our previous claim, then claim a pid THIS core may run. If the
    // requested pid is owned by ANOTHER core (a cooperative hand-off targeted a
    // thread the other core is already running), re-pick a runnable thread for
    // this core instead of panicking — this is the SMP-correct back-off that
    // makes every hand-off call site safe without per-site auditing. On a single
    // core this always resolves to `req_pid` (claim is free), so behaviour there
    // is unchanged.
    {
        let _g = PTABLE_LOCK.lock();
        // STICKY CPU AFFINITY: do NOT release `old_pid`'s claim here. Releasing it on
        // every hand-off makes every yielded thread instantly pickable by the other
        // core, so both cores chase the same hot threads and livelock (rendering
        // stalls at ~1 frame under smp=2). Keeping the claim sticky pins each thread
        // to the core that first ran it; unclaimed threads distribute across cores as
        // they become runnable, and a blocked thread is reclaimed by its own core.
        // (current_cpu is reset to None when the PCB is freed/reused on exit.)
        if my_cpu == 0 {
            crate::arch::smp::set_sched_ready();
        }
    }
    let pid = loop {
        let got = {
            let _g = PTABLE_LOCK.lock();
            let p = unsafe { &mut PTABLE[idx_of(req_pid)] };
            if p.pid == req_pid
                && (p.current_cpu.is_none() || p.current_cpu == Some(my_cpu))
            {
                p.current_cpu = Some(my_cpu);
                true
            } else {
                false
            }
        };
        if got {
            break req_pid;
        }
        // req_pid is owned by another core — pick a different runnable thread for
        // this core (next_runnable_pid claims it for my_cpu). If none, idle + retry.
        if let Some(other) = next_runnable_pid(current_pid()) {
            if other != req_pid {
                break other;
            }
        }
        unsafe { crate::arch::enable_and_halt() };
    };

    let (pml4_phys, rip, rsp, rflags, rax, rdi, rsi, rdx,
         rbx, rbp, r8, r9, r10, r11, r12, r13, r14, r15, rcx,
         syscall_stack_top, fs_base, errno_to_deliver, preempted_by_timer) = {
        let _g = PTABLE_LOCK.lock();
        let p = unsafe { &mut PTABLE[idx_of(pid)] };
        assert_eq!(p.pid, pid, "[Process] enter_user_by_pid_noreturn: stale PID");
        // Claim already established above; ensure it's ours (re-pick set it too).
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

    // aarch64: if this thread was timer-preempted at an arbitrary instruction,
    // its FULL register file lives in `arch_trapframe`. Resume from that frame
    // directly — the `EnterUserRegs`/`build_image` reconstruction below rebuilds
    // from the lossy x86-named `p.regs`, which ZEROES caller-saved x6/x7/x11..x18
    // and carries stale x24..x28. A Dart worker preempted mid heap-allocation
    // keeps live pointers (a `this`, a mutex address) in exactly those registers;
    // when a sibling's cooperative yield then schedules it through this path the
    // lossy rebuild corrupts them → the nondeterministic near-null deref in the
    // allocator. (No-op for threads that yielded via a syscall: they have no
    // valid snapshot — `arch_take_trapframe` returns None — so they fall through
    // to the register reconstruction, which is correct for that case.) This
    // mirrors the timer-ISR restore path, which already prefers this frame.
    #[cfg(target_arch = "aarch64")]
    {
        if let Some(frame) = arch_take_trapframe(pid) {
            unsafe { crate::arch::enter_user_from_frame(&frame) }
        }
    }

    // aarch64: carry the user link register (x30) in the otherwise-unused
    // `rflags` slot (build_image maps rflags→x30 on aarch64; SPSR is constant
    // there). Without this the cooperative SYSRET resume leaves x30=0 and the
    // resuming thread's next `ret` branches to address 0.
    #[cfg(target_arch = "aarch64")]
    let rflags = {
        let _g = PTABLE_LOCK.lock();
        let p = unsafe { &PTABLE[idx_of(pid)] };
        if p.pid == pid { p.user_lr } else { rflags }
    };

    // Assemble the full user register state for the arch entry hook. The
    // arch backend owns the actual ring-3 transition asm (IRETQ vs SYSRETQ on
    // x86_64); the shared code above owns all the PTABLE/CR3/errno logic.
    // aarch64: in RETURN mode the syscall's result (in rax) must be delivered in
    // x0; build_image maps rdi→x0, so override the rdi we pass. RE-EXEC mode keeps
    // rdi=arg0 (x8=rax carries the nr). See `aarch64_ret_in_x0`.
    #[cfg(target_arch = "aarch64")]
    let rdi = {
        let _g = PTABLE_LOCK.lock();
        let p = unsafe { &PTABLE[idx_of(pid)] };
        if p.pid == pid && p.aarch64_ret_in_x0 { rax } else { rdi }
    };
    #[cfg(target_arch = "aarch64")]
    let cs = callee_saved_x24_x28(pid);
    let enter_regs = crate::arch::EnterUserRegs {
        rip, rsp, rflags,
        rax, rbx, rcx, rdx, rsi, rdi, rbp,
        r8, r9, r10, r11, r12, r13, r14, r15,
        #[cfg(target_arch = "aarch64")]
        x24: cs.0,
        #[cfg(target_arch = "aarch64")]
        x25: cs.1,
        #[cfg(target_arch = "aarch64")]
        x26: cs.2,
        #[cfg(target_arch = "aarch64")]
        x27: cs.3,
        #[cfg(target_arch = "aarch64")]
        x28: cs.4,
    };

    // aarch64: the FP/SIMD file captured at SVC entry by the vector stub (before
    // kernel NEON). The full-frame path above already restores FP; this is for
    // the build_image fall-through (no full frame). None for a fresh thread.
    #[cfg(target_arch = "aarch64")]
    // Fresh threads (no saved FP) MUST start with ZEROED v0-v31/FPSR/FPCR, not the
    // kernel's leftover FP garbage — otherwise the new thread's AOT code reads an
    // uninitialized v-register → garbage value → bad Dart string ptr → "Invalid UTF8"
    // → crash (nondeterministic, early). Always restore an FP image; zeroed if none.
    #[cfg(target_arch = "aarch64")]
    let fp_buf = aarch64_take_fp(pid).unwrap_or([0u64; 66]);
    #[cfg(target_arch = "aarch64")]
    let fp_img: Option<&[u64; 66]> = Some(&fp_buf);

    if preempted_by_timer {
        // ── IRET path ────────────────────────────────────────────────────────
        // This thread was timer-preempted at an arbitrary user instruction, so
        // we MUST restore every register (including RCX, R11, and the exact
        // RFLAGS) before returning.  SYSRETQ is wrong here because it would
        // set RIP=RCX and RFLAGS=R11, corrupting those registers and losing
        // the real FLAGS state (e.g. CF/ZF from a cmp instruction).
        #[cfg(target_arch = "aarch64")]
        unsafe { crate::arch::enter_user_iret(&enter_regs, fp_img) }
        #[cfg(not(target_arch = "aarch64"))]
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
    #[cfg(target_arch = "aarch64")]
    unsafe { crate::arch::enter_user_sysret(&enter_regs, fp_img) }
    #[cfg(not(target_arch = "aarch64"))]
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
    if rip == 0 || rip < 0x1000 {
        log::error!(
            "[enter-user-ZEROPC] pid={} rip={:#x} rsp={:#x} rax={:#x} fs={:#x} preempted={}",
            pid, rip, rsp, rax, fs_base, preempted_by_timer
        );
    }
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

    // aarch64: prefer the full saved trap-frame (timer-preempt snapshot) over the
    // lossy `p.regs` reconstruction — see the matching block in
    // `enter_user_by_pid_noreturn` for the full rationale (caller-saved
    // x6/x7/x11..x18 / x24..x28 corruption of a cooperatively-scheduled
    // timer-preempted Dart worker).
    #[cfg(target_arch = "aarch64")]
    {
        if let Some(frame) = arch_take_trapframe(pid) {
            unsafe { crate::arch::enter_user_from_frame(&frame) }
        }
    }

    // aarch64: carry the user link register (x30) in the otherwise-unused
    // `rflags` slot. build_image maps rflags→x30 on aarch64; SPSR is a constant
    // there, so rflags is free. (The timer-preempt/IRET path restores x30 from
    // arch_trapframe instead, so this only matters for the SYSRET/cooperative
    // resume below.)
    #[cfg(target_arch = "aarch64")]
    let rflags = {
        let _g = PTABLE_LOCK.lock();
        let p = unsafe { &PTABLE[idx_of(pid)] };
        if p.pid == pid { p.user_lr } else { rflags }
    };

    // aarch64: in RETURN mode the syscall's result (in rax) must be delivered in
    // x0; build_image maps rdi→x0, so override the rdi we pass. RE-EXEC mode keeps
    // rdi=arg0 (x8=rax carries the nr). See `aarch64_ret_in_x0`.
    #[cfg(target_arch = "aarch64")]
    let rdi = {
        let _g = PTABLE_LOCK.lock();
        let p = unsafe { &PTABLE[idx_of(pid)] };
        if p.pid == pid && p.aarch64_ret_in_x0 { rax } else { rdi }
    };
    #[cfg(target_arch = "aarch64")]
    let cs = callee_saved_x24_x28(pid);
    let enter_regs = crate::arch::EnterUserRegs {
        rip, rsp, rflags,
        rax, rbx, rcx, rdx, rsi, rdi, rbp,
        r8, r9, r10, r11, r12, r13, r14, r15,
        #[cfg(target_arch = "aarch64")]
        x24: cs.0,
        #[cfg(target_arch = "aarch64")]
        x25: cs.1,
        #[cfg(target_arch = "aarch64")]
        x26: cs.2,
        #[cfg(target_arch = "aarch64")]
        x27: cs.3,
        #[cfg(target_arch = "aarch64")]
        x28: cs.4,
    };

    #[cfg(target_arch = "aarch64")]
    // Fresh threads (no saved FP) MUST start with ZEROED v0-v31/FPSR/FPCR, not the
    // kernel's leftover FP garbage — otherwise the new thread's AOT code reads an
    // uninitialized v-register → garbage value → bad Dart string ptr → "Invalid UTF8"
    // → crash (nondeterministic, early). Always restore an FP image; zeroed if none.
    #[cfg(target_arch = "aarch64")]
    let fp_buf = aarch64_take_fp(pid).unwrap_or([0u64; 66]);
    #[cfg(target_arch = "aarch64")]
    let fp_img: Option<&[u64; 66]> = Some(&fp_buf);

    if preempted_by_timer {
        #[cfg(target_arch = "aarch64")]
        unsafe { crate::arch::enter_user_iret(&enter_regs, fp_img) }
        #[cfg(not(target_arch = "aarch64"))]
        unsafe { crate::arch::enter_user_iret(&enter_regs) }
    }

    #[cfg(target_arch = "aarch64")]
    unsafe { crate::arch::enter_user_sysret(&enter_regs, fp_img) }
    #[cfg(not(target_arch = "aarch64"))]
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
    // SMP M1 (non-preemptable bail): never switch away while this CPU holds
    // PTABLE_LOCK — doing so would leave the lock pinned to this CPU and wedge it.
    if preempt_disabled() {
        return None;
    }
    let my_cpu = crate::arch::smp::current_cpu_id();
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

    // Save XSTATE for cur_pid (x86 only; aarch64 FP is already in cur's TrapFrame,
    // stored via arch_store_trapframe — the live v-file here is kernel garbage).
    #[cfg(not(target_arch = "aarch64"))]
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

    // Restore XSTATE for next_pid (x86 only; aarch64 restores FP via the trap
    // frame on eret / enter_user_from_frame).
    #[cfg(not(target_arch = "aarch64"))]
    {
        let ptr_next = p_next.xstate.0.as_ptr();
        unsafe { crate::arch::cpu::restore_xstate_from(ptr_next) };
    }

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
    // SMP M1 (non-preemptable bail): if the interrupted thread holds PTABLE_LOCK,
    // a recursive try_lock below would SUCCEED and let us switch threads mid-
    // critical-section, pinning the lock to this CPU forever. Bail instead.
    if preempt_disabled() {
        return None;
    }
    let my_cpu = crate::arch::smp::current_cpu_id();
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

        // Save XSTATE for cur_pid (x86 only; aarch64 FP is already in cur's
        // TrapFrame, stored via arch_store_trapframe).
        #[cfg(not(target_arch = "aarch64"))]
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

        // Restore XSTATE for next_pid (x86 only; aarch64 restores FP via the
        // trap frame on eret / enter_user_from_frame).
        #[cfg(not(target_arch = "aarch64"))]
        {
            let ptr_next = p_next.xstate.0.as_ptr();
            unsafe { crate::arch::cpu::restore_xstate_from(ptr_next) };
        }

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
/// Length in bytes of the user syscall-entry instruction. On x86_64 `syscall`
/// is 2 bytes; on aarch64 `svc #0` is 4 bytes. After a syscall the user PC
/// (ELR_EL1 / the saved RIP) points at the *next* instruction, so to make a
/// blocked thread RE-EXECUTE its syscall on resume the kernel rewinds the saved
/// PC by exactly one syscall instruction.
#[cfg(target_arch = "x86_64")]
pub const SYSCALL_INSN_LEN: u64 = 2;
#[cfg(target_arch = "aarch64")]
pub const SYSCALL_INSN_LEN: u64 = 4;

/// Save a thread's resume context rewound to RE-EXECUTE the syscall it is
/// currently in (used by the blocking syscalls — epoll_wait/poll/wm_event_wait/
/// futex — which resume by re-entering the kernel and re-checking readiness).
/// Rewinds the saved PC by one syscall instruction (arch-correct length).
/// Using a hardcoded `-2` here mis-rewinds aarch64's 4-byte `svc` into the
/// middle of the instruction → a PC-alignment fault (EC=0x22) on resume.
pub fn save_return_context_reexec(pid: u32, urip: u64, ursp: u64) {
    save_return_context_inner(pid, urip.wrapping_sub(SYSCALL_INSN_LEN), ursp, SYSCALL_INSN_LEN);
}

pub fn save_return_context(pid: u32, rip: u64, rsp: u64) {
    save_return_context_inner(pid, rip, rsp, 0);
}

/// `aarch64_rewind` is how far the resume PC is rewound from the syscall return
/// point (0 for a plain return, SYSCALL_INSN_LEN to re-execute the `svc`). On
/// aarch64 the resume PC/SP/LR come from the TARGET thread's eagerly-captured
/// entry snapshot (`entry_user_rip`/`entry_user_rsp`/`user_lr`, taken in the
/// IRQ-masked SVC window), NOT from the caller's `rip`/`rsp` — those were read
/// from the live per-CPU scratch, which on the wake path belongs to the WAKER
/// (not `pid`) and on a self-yield can be clobbered by a timer-preempted
/// sibling's SVC. Trusting the scratch there resumed the thread on the wrong SP,
/// so its `ldp x29,x30,[sp,#..]; ret` read a stale/zero frame record → `ret` 0.
fn save_return_context_inner(pid: u32, rip: u64, rsp: u64, aarch64_rewind: u64) {
    let fs = crate::arch::cpu::get_fs_base();
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx_of(pid)] };
    if p.pid == pid {
        #[cfg(target_arch = "aarch64")]
        {
            // Prefer this thread's own entry-captured PC/SP; fall back to the
            // passed values only if no entry capture exists yet (e.g. a fresh
            // thread that has not run a syscall). x30 was already persisted by
            // the eager capture — do NOT overwrite it with the live scratch.
            if p.entry_user_rip != 0 {
                p.regs.rip = p.entry_user_rip.wrapping_sub(aarch64_rewind);
                p.regs.rsp = p.entry_user_rsp;
            } else {
                p.regs.rip = rip;
                p.regs.rsp = rsp;
            }
            // rewind==0 → RETURN mode (PC after the `svc`): the syscall's return
            // value (set via set_rax) must be delivered in x0 on resume. rewind!=0
            // → RE-EXEC mode (PC at the `svc`): x0 must stay arg0 and x8 carry the
            // nr, so do NOT override x0. See the field doc on `aarch64_ret_in_x0`.
            p.aarch64_ret_in_x0 = aarch64_rewind == 0;
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            let _ = aarch64_rewind;
            p.regs.rip = rip;
            p.regs.rsp = rsp;
        }
        p.regs.rflags = 0x202; // IF=1, standard user RFLAGS
        // Saved at a SYSCALL boundary — SYSRETQ is correct for this thread.
        p.preempted_by_timer = false;
        p.fs_base = fs;
    }
}

/// Guards against a since-clobbered per-CPU user-GPR snapshot leaking into a
/// thread's saved context.
///
/// The user GPR snapshot lives in a PER-CPU scratch area, written at syscall
/// entry from the trapping thread's frame. It is shared by every thread on that
/// core, so once a syscall hands off the CPU (a cooperative `epoll`/`futex`/
/// `cond` yield, or — on aarch64 — a generic-timer preempt that lands a
/// sibling's SVC mid-handler), another thread's syscall entry OVERWRITES it. A
/// later `save_full_user_gprs` would then read the *other* thread's callee-saved
/// regs / link register and store them into our context → on resume rbx/rbp/
/// r12–15 (x19–x23/x29) or x30 are garbage, and the next `ret` branches to a
/// stale address (observed on aarch64: x30=0 → `ret` to 0 → EL0 UDF after
/// `FlutterEngineRunInitialized`; the x86 analogue corrupted `MessageLoop::Run`'s
/// `this`).
///
/// Fix: capture ONCE, eagerly, at syscall entry (`capture_user_gprs_at_entry`),
/// while the snapshot is still fresh for THIS thread (x86: interrupts masked
/// until the handler `sti`s; aarch64: from inside the IRQ-masked SVC window in
/// the vector dispatch). This flag makes every subsequent yield-time
/// `save_full_user_gprs` in the same syscall a no-op, so a clobbered per-CPU
/// snapshot can never leak into our saved context.
const GPR_CAP_MAX_CPUS: usize = 64;
static GPRS_CAPTURED: [core::sync::atomic::AtomicBool; GPR_CAP_MAX_CPUS] =
    [const { core::sync::atomic::AtomicBool::new(false) }; GPR_CAP_MAX_CPUS];

#[inline]
fn gpr_cap_cpu_idx() -> usize {
    #[cfg(target_arch = "aarch64")]
    { (crate::arch::aarch64::smp::current_cpu_id() as usize).min(GPR_CAP_MAX_CPUS - 1) }
    #[cfg(not(target_arch = "aarch64"))]
    { (crate::arch::smp::this_cpu().cpu_id as usize).min(GPR_CAP_MAX_CPUS - 1) }
}

/// Capture the user GPRs into `pid`'s PTABLE slot at syscall ENTRY, while the
/// per-CPU snapshot is guaranteed fresh for this thread. Marks them captured so
/// later `save_full_user_gprs` calls this syscall don't re-read the (possibly
/// since-clobbered) shared per-CPU snapshot. MUST be invoked while the snapshot
/// cannot yet be overwritten by a sibling (IRQs masked / pre-`sti`).
pub fn capture_user_gprs_at_entry(pid: u32) {
    GPRS_CAPTURED[gpr_cap_cpu_idx()].store(false, core::sync::atomic::Ordering::Relaxed);
    save_full_user_gprs(pid);
}

/// Snapshot the full user GPR set (as captured at SYSCALL entry) into the
/// process's register file. Must be called from inside a syscall handler
/// before yielding the CPU to another process via
/// `enter_user_by_pid_noreturn`, otherwise the yielding thread will resume
/// with stale rbx/rbp/r12–r15/rdi/rsi/etc. and corrupt its C++ caller's
/// `this` pointer and locals.
///
/// The user GPR snapshot lives in a PER-CPU scratch area (`gs:[..]`), written by
/// the syscall entry stub. It is shared by every thread on that core, so if a
/// thread switch lands mid-handler (the wait loops `sti; hlt` and the timer ISR
/// can switch threads), another thread's syscall entry OVERWRITES it. A later
/// `save_full_user_gprs` would then read the *other* thread's callee-saved regs
/// and store them into our context → on resume `rbx`/`rbp`/`r12-15` are garbage
/// (observed: `MessageLoopOscortex::Run` resuming from `epoll_wait` with
/// `rbx`=a Skia stride → SIGSEGV; the root of the "sporadic" render crashes).
///
/// Fix: capture ONCE, eagerly, at syscall entry (`capture_user_gprs_at_entry`,
/// called from `dispatch_fast` while the snapshot is still fresh for THIS
/// thread). This flag makes every subsequent yield-time `save_full_user_gprs`
/// in the same syscall a no-op, so a clobbered per-CPU snapshot can never leak
/// into our saved context.
pub fn save_full_user_gprs(pid: u32) {
    // Only the first call per syscall (the eager entry capture) reads the
    // per-CPU snapshot; it is fresh then. Later yield-time calls are no-ops so a
    // since-clobbered snapshot cannot leak into our saved context.
    if GPRS_CAPTURED[gpr_cap_cpu_idx()].swap(true, core::sync::atomic::Ordering::Relaxed) {
        return;
    }
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
    // aarch64: persist the user link register (x30) so the thread can `ret`
    // correctly after resuming from this syscall yield, plus the entry PC/SP so a
    // blocking syscall's save_return_context can rewind from the FRESH entry
    // values rather than the live (clobberable) per-CPU scratch.
    #[cfg(target_arch = "aarch64")]
    {
        p.user_lr = snap.lr;
        p.entry_user_rip = crate::arch::syscall::user_rip();
        p.entry_user_rsp = crate::arch::syscall::user_rsp();
        p.user_x24 = snap.x24;
        p.user_x25 = snap.x25;
        p.user_x26 = snap.x26;
        p.user_x27 = snap.x27;
        p.user_x28 = snap.x28;
    }
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

/// Read the saved user link register (x30) for a pid.
#[cfg(target_arch = "aarch64")]
pub fn get_user_lr(pid: u32) -> u64 {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &PTABLE[idx_of(pid)] };
    if p.pid == pid { p.user_lr } else { 0 }
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
    // A forked child inherits the parent's capability set.
    child.caps               = unsafe { core::ptr::addr_of!(PTABLE[idx_of(parent_pid)].caps).read() };
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
