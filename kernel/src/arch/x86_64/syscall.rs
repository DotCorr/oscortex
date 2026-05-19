//! SYSCALL/SYSRET fast path — x86_64.
//!
//! Modern syscalls go through SYSCALL instruction (ring 3 → ring 0) and return
//! via SYSRET. This is significantly faster than INT 0x80.
//!
//! ## Stack model
//!
//! SYSCALL does NOT switch stacks. On entry RSP still points to the user stack.
//! We immediately save user RSP to a per-BSP scratch slot and switch to a
//! dedicated 8 KiB kernel syscall stack before touching anything else.
//! Before SYSRET we restore user RSP from the same scratch slot.
//!
//! ## STAR / SYSRET layout
//!
//! GDT layout (see gdt.rs):
//!   0x18 — user CS32 placeholder  (STAR base)
//!   0x20 — user DS                (SYSRET SS  = base + 8  | RPL3 = 0x23)
//!   0x28 — user CS64              (SYSRET CS  = base + 16 | RPL3 = 0x2B)
//!
//! So STAR[63:48] = 0x18.

use super::gdt::KERNEL_CS;
use core::arch::asm;

const STAR_MSR:  u32 = 0xC000_0081;
const LSTAR_MSR: u32 = 0xC000_0082;
const FMASK_MSR: u32 = 0xC000_0084;
const EFER_MSR:  u32 = 0xC000_0080;

/// SYSRET64 GDT base: STAR[63:48]. CS = base+16|3, SS = base+8|3.
const USER_SYSRET_BASE: u64 = 0x18;

// ── Per-BSP kernel syscall stack (8 KiB, 16-byte aligned) ────────────────────

/// Dedicated kernel stack for syscall handlers.
/// Single-CPU kernel — no per-CPU replication needed yet.
#[repr(C, align(16))]
struct SyscallStack([u8; 8192]);

static mut SYSCALL_KERNEL_STACK: SyscallStack = SyscallStack([0; 8192]);

/// Cached pointer to the TOP of `SYSCALL_KERNEL_STACK` (set once in `init()`).
/// Stored as a plain u64 so naked_asm can load it with a single `mov rsp, [..]`.
static mut SYSCALL_STACK_TOP: u64 = 0;
static mut ACTIVE_SYSCALL_STACK_TOP: u64 = 0;

/// Scratch slot to save user RSP across the syscall handler.
/// Single-CPU only — safe because SYSCALL cannot nest with itself.
static mut SYSCALL_USER_RSP: u64 = 0;

/// Scratch slot to save user RIP (RCX at SYSCALL entry) — used by clone(2).
static mut SYSCALL_USER_RIP: u64 = 0;

/// Scratch slot to save user R9 (6th SysV arg, often the 3rd vararg of a
/// 3-fixed-arg variadic such as snprintf). The shuffle in `syscall_entry`
/// overwrites r9 with the linux a4, so we stash it here first.
static mut SYSCALL_USER_R9: u64 = 0;

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    unsafe {
        // Cache the kernel syscall stack top once.
        SYSCALL_STACK_TOP =
            SYSCALL_KERNEL_STACK.0.as_ptr().add(8192) as u64;
        ACTIVE_SYSCALL_STACK_TOP = SYSCALL_STACK_TOP;

        // Enable SYSCALL/SYSRET via EFER.SCE bit.
        let efer: u64;
        asm!("rdmsr", in("ecx") EFER_MSR, out("eax") efer, out("edx") _, options(nomem, nostack));
        wrmsr(EFER_MSR, efer | 1); // SCE = 1

        // STAR[47:32] = KERNEL_CS (for SYSCALL)
        // STAR[63:48] = USER_SYSRET_BASE (for SYSRET64)
        let star = (USER_SYSRET_BASE << 48) | ((KERNEL_CS as u64) << 32);
        wrmsr(STAR_MSR, star);

        // LSTAR: fast syscall entry point.
        wrmsr(LSTAR_MSR, syscall_entry as *const () as u64);

        // FMASK: mask IF on entry (keep interrupts disabled during syscall entry).
        wrmsr(FMASK_MSR, 0x200);
    }
    log::info!("[Syscall] SYSCALL/SYSRET initialised (STAR base={:#x})", USER_SYSRET_BASE);
}

pub fn set_active_stack_top(stack_top: u64) {
    unsafe {
        ACTIVE_SYSCALL_STACK_TOP = if stack_top == 0 {
            SYSCALL_STACK_TOP
        } else {
            stack_top
        };
    }
}

/// Return the user RSP that was live when the current syscall was entered.
pub fn user_rsp() -> u64 {
    unsafe { SYSCALL_USER_RSP }
}

/// Return the user RIP (return address) that was live when the current syscall was entered.
pub fn user_rip() -> u64 {
    unsafe { SYSCALL_USER_RIP }
}

/// Return user R9 captured at SYSCALL entry (before the dispatch shuffle).
pub fn user_r9() -> u64 {
    unsafe { SYSCALL_USER_R9 }
}

/// Fast syscall entry point (SYSCALL instruction from ring 3).
///
/// On entry (hardware convention):
///   RAX = syscall number
///   RCX = user RIP  (saved by SYSCALL)
///   R11 = user RFLAGS (saved by SYSCALL)
///   RDI = arg0, RSI = arg1, RDX = arg2, R10 = arg3
///   RSP = user stack  ← we must NOT use this for kernel work
///
/// We call: `dispatch_fast(number, arg0, arg1, arg2)` — SysV ABI:
///   rdi = number, rsi = arg0, rdx = arg1, rcx = arg2
///
/// Rearrangement:
///   rcx ← rdx   (arg2;  rcx is free because user RIP was saved by SYSCALL)
///   rdx ← rsi   (arg1)
///   rsi ← rdi   (arg0)
///   rdi ← rax   (number)
#[unsafe(naked)]
unsafe extern "C" fn syscall_entry() {
    core::arch::naked_asm!(
        // ── 1. Switch to kernel stack ─────────────────────────────────────
        "mov [{user_rsp}], rsp",          // save user RSP
        "mov [{user_rip}], rcx",          // save user RIP (RCX = return addr from SYSCALL)
        "mov [{user_r9}], r9",            // save user R9 (often 3rd vararg)
        "mov rsp, [{kstack_top}]",        // load kernel syscall stack top

        // ── 2. Save user RIP, RFLAGS, and all caller-saved arg regs ───────
        // The Linux syscall ABI guarantees that only rax/rcx/r11 change
        // across a syscall.  All other registers (rdi, rsi, rdx, r10, r8,
        // r9) must be visible to user code with their original values after
        // we SYSRET.  Push them now so we can pop them back after dispatch.
        "push rcx",                       // user RIP
        "push r11",                       // user RFLAGS
        "push rdi",                       // user arg0 (rdi)
        "push rsi",                       // user arg1 (rsi)
        "push rdx",                       // user arg2 (rdx)
        "push r10",                       // user arg3 (r10)
        "push r8",                        // user arg4 (r8)
        "push r9",                        // user arg5 (r9)

        // ── 3. Rearrange for dispatch_fast(number, a0, a1, a2, a3, a4) ───
        // SysV arg order: rdi=number, rsi=a0, rdx=a1, rcx=a2, r8=a3, r9=a4
        // Linux syscall:  rdi=a0, rsi=a1, rdx=a2, r10=a3, r8=a4, rax=number
        "mov r9,  r8",                    // a4 = user r8
        "mov r8,  r10",                   // a3 = user r10
        "mov rcx, rdx",                   // a2 = user rdx
        "mov rdx, rsi",                   // a1 = user rsi
        "mov rsi, rdi",                   // a0 = user rdi
        "mov rdi, rax",                   // number = syscall number

        // ── 4. Dispatch ───────────────────────────────────────────────────
        "call {dispatch}",
        // rax now holds the syscall return value (SysV: return in rax).

        // ── 5. Restore user registers and return ──────────────────────────
        "pop r9",
        "pop r8",
        "pop r10",
        "pop rdx",
        "pop rsi",
        "pop rdi",
        "pop r11",                        // user RFLAGS
        "pop rcx",                        // user RIP
        "mov rsp, [{user_rsp}]",          // restore user RSP
        "sysretq",

        dispatch   = sym crate::syscall::dispatch_fast,
        user_rsp   = sym SYSCALL_USER_RSP,
        user_rip   = sym SYSCALL_USER_RIP,
        user_r9    = sym SYSCALL_USER_R9,
        kstack_top = sym ACTIVE_SYSCALL_STACK_TOP,
    )
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn wrmsr(msr: u32, val: u64) {
    unsafe {
        asm!("wrmsr",
            in("ecx") msr,
            in("eax") val as u32,
            in("edx") (val >> 32) as u32,
            options(nomem, nostack),
        );
    }
}

