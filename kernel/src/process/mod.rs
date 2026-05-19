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
static PENDING_INIT_PID: AtomicU32 = AtomicU32::new(0);

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum simultaneous live processes.
pub const MAX_PROCS: usize = 256;

/// ELF load base virtual address (user space).
pub const USER_ELF_BASE: u64 = 0x0000_0000_0040_0000; // 4 MiB

/// User stack top (grows downward; bottom = TOP − STACK_SIZE).
pub const USER_STACK_TOP:  u64  = 0x0000_7FFF_FFFF_0000;
pub const USER_STACK_SIZE: usize = 64 * 1024; // 64 KiB
const SYSCALL_STACK_SIZE: usize = 8 * 1024;
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
        Self([0; XSTATE_SIZE])
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

/// Returns true if `pid` is a pthread (clone-thread) of another process.
/// Used by the syscall FS-bootstrap to skip auto-assigning a fake FS base
/// for new threads — pthread runtimes set their own FS via arch_prctl.
pub fn is_thread(pid: u32) -> bool {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &PTABLE[idx_of(pid)] };
    p.pid == pid && p.is_thread
}

// ── Global process table ──────────────────────────────────────────────────────

// Safety: all mutations are protected by `PTABLE_LOCK`.
static mut PTABLE: [Process; MAX_PROCS] = {
    // SAFETY: Process::empty() is a const fn, array is zero-initialised.
    [const { Process::empty() }; MAX_PROCS]
};
static PTABLE_LOCK: Mutex<()> = Mutex::new(());

/// Next PID to hand out (starts at 1; PID 0 is the kernel).
static NEXT_PID: AtomicU32 = AtomicU32::new(1);
/// PID currently bound to the active userspace context on this CPU.
///
/// Today this is a global fallback (single active userspace context path).
/// Per-CPU PID binding comes with full user scheduler integration.
static CURRENT_PID: AtomicU32 = AtomicU32::new(0);

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

fn idx_of(pid: u32) -> usize {
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

/// Spawn a new process from an ELF image in memory.
///
/// Returns the new PID, or an error string.
pub fn spawn(elf_bytes: &[u8], name: &str) -> Result<u32, &'static str> {
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
    posix_trampolines::map_system_pages(pml4_phys)?;

    let idx = idx_of(pid);
    let mut regs = UserRegs::default();
    regs.rip    = entry;
    regs.rsp    = USER_STACK_TOP - 8; // leave one guard word
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
        p.parent_pid = 0;
        p.fs_base    = 0;
    }

    log::info!("[Process] Spawned '{}' pid={} entry={:#x}", name, pid, entry);
    Ok(pid)
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
    // the parent's PML4 and must never free it.
    if !p.is_thread {
        paging::free_user_pml4(p.pml4_phys);
    }
    free_syscall_stack(p.syscall_stack_base);
    p.syscall_stack_base = core::ptr::null_mut();
    p.syscall_stack_top = 0;

    p.state     = ProcState::Zombie(code);
    p.exit_code = code;
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
    CURRENT_PID.store(pid, Ordering::Release);
}

/// Get currently active userspace PID.
pub fn current_pid() -> u32 {
    CURRENT_PID.load(Ordering::Acquire)
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
pub fn save_xstate(pid: u32) {
    let idx = idx_of(pid);
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx] };
    if p.pid != pid || p.state != ProcState::Running {
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

/// Round-robin pick of next runnable process after `current`.
pub fn next_runnable_pid(current: u32) -> Option<u32> {
    let _g = PTABLE_LOCK.lock();

    let mut start = idx_of(current.wrapping_add(1));
    if current == 0 {
        start = 0;
    }

    for off in 0..MAX_PROCS {
        let idx = (start + off) % MAX_PROCS;
        let p = unsafe { &PTABLE[idx] };
        if p.pid != 0 && p.state == ProcState::Running {
            if current == 0 || p.pid != current {
                return Some(p.pid);
            }
        }
    }

    if current != 0 {
        let c = unsafe { &PTABLE[idx_of(current)] };
        if c.pid == current && c.state == ProcState::Running {
            return Some(current);
        }
    }

    None
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
    let parent_pml4_phys = {
        let _g = PTABLE_LOCK.lock();
        let p = unsafe { &PTABLE[idx_of(parent_pid)] };
        if p.pid != parent_pid || p.state == ProcState::Dead {
            log::error!("[spawn_thread] parent not found pid={}", parent_pid);
            return Err("parent not found");
        }
        p.pml4_phys
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
    unsafe { core::ptr::write_volatile(tls_va as *mut u64, tls_va); }

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
        p.parent_pid         = parent_pid;
        p.fs_base            = tls_va;
    }

    log::info!("[Process] Thread tid={} spawned in pid={} entry={:#x} tls={:#x}",
        tid, parent_pid, entry_fn, tls_va);
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
    let parent_pml4_phys = {
        let _g = PTABLE_LOCK.lock();
        let p = unsafe { &PTABLE[idx_of(parent_pid)] };
        if p.pid != parent_pid || p.state == ProcState::Dead {
            return Err("parent not found");
        }
        p.pml4_phys
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
        p.parent_pid         = parent_pid;
        p.fs_base            = 0;
    }

    log::info!("[Process] clone_thread tid={} in pid={} rip={:#x}", tid, parent_pid, child_rip);
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
    let (pml4_phys, rip, rsp, rflags, rax, rdi, rsi, rdx,
         rbx, rbp, r8, r9, r10, r12, r13, r14, r15,
         syscall_stack_top, fs_base) = {
        let _g = PTABLE_LOCK.lock();
        let p = unsafe { &PTABLE[idx_of(pid)] };
        assert_eq!(p.pid, pid, "[Process] enter_user_by_pid_noreturn: stale PID");
        (p.pml4_phys, p.regs.rip, p.regs.rsp, p.regs.rflags, p.regs.rax,
         p.regs.rdi, p.regs.rsi, p.regs.rdx,
         p.regs.rbx, p.regs.rbp, p.regs.r8, p.regs.r9, p.regs.r10,
         p.regs.r12, p.regs.r13, p.regs.r14, p.regs.r15,
         p.syscall_stack_top, p.fs_base)
    };

    log::warn!(
        "[trace] enter_user pid={} rip={:#x} rsp={:#x} rax={:#x} fs={:#x} cr3={:#x}",
        pid, rip, rsp, rax, fs_base, pml4_phys
    );

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
    unsafe {
        core::arch::asm!(
            "mov cr3, {cr3}",
            cr3 = in(reg) pml4_phys,
            options(nostack, nomem, preserves_flags),
        );
    }

    // Use SYSRET to enter user mode. This is the native x86_64 user-entry path
    // and mirrors how SYSCALL returns to user code. The full user GPR set is
    // restored so a thread that yielded via a syscall (e.g. futex_wait) sees
    // exactly the register state it had at the SYSCALL instant, with only
    // RAX changed to the syscall return value. This is required because the
    // SysV ABI mandates that callee-saved regs (rbx, rbp, r12–r15) survive
    // a function call, and the userspace SYSCALL trampoline is such a call.
    //
    // Stage values in a fixed-layout stack array and load them inside the asm
    // block via a single pointer. RSP and the syscall-consumed regs (RCX,
    // R11) are loaded last so all intermediate reads complete first.
    let frame: [u64; 16] = [
        /* 0x00 */ rip,
        /* 0x08 */ rflags,
        /* 0x10 */ rsp,
        /* 0x18 */ rdi,
        /* 0x20 */ rsi,
        /* 0x28 */ rdx,
        /* 0x30 */ rax,
        /* 0x38 */ rbx,
        /* 0x40 */ rbp,
        /* 0x48 */ r8,
        /* 0x50 */ r9,
        /* 0x58 */ r10,
        /* 0x60 */ r12,
        /* 0x68 */ r13,
        /* 0x70 */ r14,
        /* 0x78 */ r15,
    ];
    unsafe {
        core::arch::asm!(
            // Pin the frame base in r11. r11 is consumed by SYSRET (as user
            // RFLAGS) and is the LAST register we load — every other load
            // can therefore use [r11 + disp] safely. If we let the compiler
            // pick the base register, it can alias the base with the first
            // destination (e.g. rax) and have the very first `mov` clobber
            // the pointer before any further reads happen.
            "mov rax, [r11 + 0x30]",
            "mov rdi, [r11 + 0x18]",
            "mov rsi, [r11 + 0x20]",
            "mov rdx, [r11 + 0x28]",
            "mov rbx, [r11 + 0x38]",
            "mov rbp, [r11 + 0x40]",
            "mov r8,  [r11 + 0x48]",
            "mov r9,  [r11 + 0x50]",
            "mov r10, [r11 + 0x58]",
            "mov r12, [r11 + 0x60]",
            "mov r13, [r11 + 0x68]",
            "mov r14, [r11 + 0x70]",
            "mov r15, [r11 + 0x78]",
            "mov rcx, [r11 + 0x00]",   // user RIP   → RCX (consumed by sysretq)
            "mov rsp, [r11 + 0x10]",   // switch to user stack (mapped in user PML4)
            "mov r11, [r11 + 0x08]",   // user RFL  → R11 (consumed by sysretq) — load LAST
            "sysretq",
            in("r11") frame.as_ptr(),
            options(noreturn),
        )
    }
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
    unsafe { core::arch::asm!("cli", options(nomem, nostack, preserves_flags)); }
    // Mark this kernel task as Zombie before we SYSRET.  The SYSRET transfers
    // execution to ring-3 permanently; this task slot must never be resumed as
    // a kernel task again.  Marking it Zombie prevents the scheduler from
    // saving a corrupt interrupt-stack RSP into our kernel_sp on subsequent
    // preemptions, and prevents the round-robin from ever selecting it again.
    crate::sched::mark_current_zombie();
    enter_user_by_pid_noreturn(pid)
}

// ── Phase 47: parent-blocking helpers ────────────────────────────────────────

/// Save the user-space return context (RIP + RSP after syscall) into the
/// process's register file so `enter_user_by_pid_noreturn` resumes correctly.
pub fn save_return_context(pid: u32, rip: u64, rsp: u64) {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx_of(pid)] };
    if p.pid == pid {
        p.regs.rip    = rip;
        p.regs.rsp    = rsp;
        p.regs.rflags = 0x202; // IF=1, standard user RFLAGS
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

/// Force a process into the `Blocked` or `Running` state.
pub fn set_state(pid: u32, state: ProcState) {
    let _g = PTABLE_LOCK.lock();
    let p = unsafe { &mut PTABLE[idx_of(pid)] };
    if p.pid == pid { p.state = state; }
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
    let parent_pid = CURRENT_PID.load(Ordering::Relaxed);
    if parent_pid == 0 { return Err("fork from kernel"); }

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

