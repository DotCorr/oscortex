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

// ── Per-CPU kernel syscall scratch and stack tables ──────────────────────────

const MAX_CPUS: usize = 64;

#[repr(C)]
pub struct CpuScratch {
    pub syscall_user_rsp: u64,  // offset 0
    pub syscall_user_rip: u64,  // offset 8
    pub syscall_user_r9: u64,   // offset 16
    pub syscall_stack_top: u64, // offset 24
    pub user_rdi: u64,          // offset 32
    pub user_rsi: u64,          // offset 40
    pub user_rdx: u64,          // offset 48
    pub user_r10: u64,          // offset 56
    pub user_r8: u64,           // offset 64
    pub user_rbx: u64,          // offset 72
    pub user_rbp: u64,          // offset 80
    pub user_r12: u64,          // offset 88
    pub user_r13: u64,          // offset 96
    pub user_r14: u64,          // offset 104
    pub user_r15: u64,          // offset 112
    pub cpu_id: u64,            // offset 120
}

#[repr(C, align(16))]
struct SyscallStack([u8; 8192]);

static mut SYSCALL_KERNEL_STACKS: [SyscallStack; MAX_CPUS] = [const { SyscallStack([0; 8192]) }; MAX_CPUS];
static mut SYSCALL_STACK_TOPS: [u64; MAX_CPUS] = [0; MAX_CPUS];
pub static mut CPU_SCRATCHES: [CpuScratch; MAX_CPUS] = [const { CpuScratch {
    syscall_user_rsp: 0,
    syscall_user_rip: 0,
    syscall_user_r9: 0,
    syscall_stack_top: 0,
    user_rdi: 0,
    user_rsi: 0,
    user_rdx: 0,
    user_r10: 0,
    user_r8: 0,
    user_rbx: 0,
    user_rbp: 0,
    user_r12: 0,
    user_r13: 0,
    user_r14: 0,
    user_r15: 0,
    cpu_id: 0,
} }; MAX_CPUS];

// ── Public API ────────────────────────────────────────────────────────────────

#[inline(always)]
pub fn current_cpu_id() -> u32 {
    crate::arch::x86_64::smp::current_cpu_id()
}

pub fn init() {
    unsafe {
        for i in 0..MAX_CPUS {
            SYSCALL_STACK_TOPS[i] =
                SYSCALL_KERNEL_STACKS[i].0.as_ptr().add(8192) as u64;
            CPU_SCRATCHES[i].syscall_stack_top = SYSCALL_STACK_TOPS[i];
            CPU_SCRATCHES[i].cpu_id = i as u64;
        }

        // Setup GS base MSRs for CPU 0 (BSP).
        let ptr = core::ptr::addr_of_mut!(CPU_SCRATCHES[0]) as u64;
        wrmsr(0xC000_0101, ptr); // IA32_GS_BASE
        wrmsr(0xC000_0102, ptr); // IA32_KERNEL_GS_BASE

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

pub fn init_ap(cpu_idx: u32) {
    unsafe {
        let ptr = core::ptr::addr_of_mut!(CPU_SCRATCHES[cpu_idx as usize]) as u64;
        wrmsr(0xC000_0101, ptr); // IA32_GS_BASE
        wrmsr(0xC000_0102, ptr); // IA32_KERNEL_GS_BASE

        // Enable SYSCALL/SYSRET via EFER.SCE bit.
        let efer: u64;
        asm!("rdmsr", in("ecx") EFER_MSR, out("eax") efer, out("edx") _, options(nomem, nostack));
        wrmsr(EFER_MSR, efer | 1); // SCE = 1

        let star = (USER_SYSRET_BASE << 48) | ((KERNEL_CS as u64) << 32);
        wrmsr(STAR_MSR, star);

        wrmsr(LSTAR_MSR, syscall_entry as *const () as u64);
        wrmsr(FMASK_MSR, 0x200);
    }
    log::info!("[Syscall] AP cpu={} SYSCALL/SYSRET initialised", cpu_idx);
}

pub fn set_active_stack_top(stack_top: u64) {
    unsafe {
        let cpu_idx = crate::arch::x86_64::smp::this_cpu().cpu_id;
        CPU_SCRATCHES[cpu_idx as usize].syscall_stack_top = if stack_top == 0 {
            SYSCALL_STACK_TOPS[cpu_idx as usize]
        } else {
            stack_top
        };
    }
}

/// Return the user RSP that was live when the current syscall was entered.
pub fn user_rsp() -> u64 {
    unsafe {
        let cpu_idx = crate::arch::x86_64::smp::this_cpu().cpu_id;
        CPU_SCRATCHES[cpu_idx as usize].syscall_user_rsp
    }
}

/// Return the user RIP (return address) that was live when the current syscall was entered.
pub fn user_rip() -> u64 {
    unsafe {
        let cpu_idx = crate::arch::x86_64::smp::this_cpu().cpu_id;
        CPU_SCRATCHES[cpu_idx as usize].syscall_user_rip
    }
}

/// Return user R9 captured at SYSCALL entry (before the dispatch shuffle).
pub fn user_r9() -> u64 {
    unsafe {
        let cpu_idx = crate::arch::x86_64::smp::this_cpu().cpu_id;
        CPU_SCRATCHES[cpu_idx as usize].syscall_user_r9
    }
}

/// Return user RBP captured at SYSCALL entry.
pub fn user_rbp() -> u64 {
    unsafe {
        let cpu_idx = crate::arch::x86_64::smp::this_cpu().cpu_id;
        CPU_SCRATCHES[cpu_idx as usize].user_rbp
    }
}

/// Snapshot of all general-purpose user registers captured at SYSCALL entry.
#[derive(Clone, Copy, Default)]
pub struct UserGprSnapshot {
    pub rdi: u64, pub rsi: u64, pub rdx: u64,
    pub r10: u64, pub r8:  u64, pub r9:  u64,
    pub rbx: u64, pub rbp: u64,
    pub r12: u64, pub r13: u64, pub r14: u64, pub r15: u64,
}

/// Read the full user GPR snapshot stashed by `syscall_entry`.
pub fn user_gprs() -> UserGprSnapshot {
    unsafe {
        let cpu_idx = crate::arch::x86_64::smp::this_cpu().cpu_id;
        let s = &CPU_SCRATCHES[cpu_idx as usize];
        UserGprSnapshot {
            rdi: s.user_rdi,
            rsi: s.user_rsi,
            rdx: s.user_rdx,
            r10: s.user_r10,
            r8:  s.user_r8,
            r9:  s.syscall_user_r9,
            rbx: s.user_rbx,
            rbp: s.user_rbp,
            r12: s.user_r12,
            r13: s.user_r13,
            r14: s.user_r14,
            r15: s.user_r15,
        }
    }
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
        "swapgs",
        "mov gs:[0], rsp",                // save user RSP (offset 0)
        "mov gs:[8], rcx",                // save user RIP (offset 8)
        "mov gs:[16], r9",                // save user R9 (offset 16)
        
        "mov gs:[32], rdi",               // user_rdi
        "mov gs:[40], rsi",               // user_rsi
        "mov gs:[48], rdx",               // user_rdx
        "mov gs:[56], r10",               // user_r10
        "mov gs:[64], r8",                // user_r8
        "mov gs:[72], rbx",               // user_rbx
        "mov gs:[80], rbp",               // user_rbp
        "mov gs:[88], r12",               // user_r12
        "mov gs:[96], r13",               // user_r13
        "mov gs:[104], r14",              // user_r14
        "mov gs:[112], r15",              // user_r15
        
        "mov rsp, gs:[24]",               // load active syscall stack top (offset 24)

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
        "mov rsp, gs:[0]",                // restore user RSP
        "swapgs",
        "sysretq",

        dispatch   = sym crate::syscall::dispatch_fast,
    )
}

// ── Legacy INT 0x80 path ─────────────────────────────────────────────────────

/// INT 0x80 entry — saves GPRs, dispatches via `dispatch_fast`, returns in RAX.
#[unsafe(naked)]
pub unsafe extern "C" fn legacy_syscall_entry() {
    core::arch::naked_asm!(
        "push r15",
        "push r14",
        "push r13",
        "push r12",
        "push r11",
        "push r10",
        "push r9",
        "push r8",
        "push rbp",
        "push rdi",
        "push rsi",
        "push rdx",
        "push rcx",
        "push rbx",
        "push rax",
        "mov rdi, rsp",
        "call {wrapper}",
        "pop rax",
        "pop rbx",
        "pop rcx",
        "pop rdx",
        "pop rsi",
        "pop rdi",
        "pop rbp",
        "pop r8",
        "pop r9",
        "pop r10",
        "pop r11",
        "pop r12",
        "pop r13",
        "pop r14",
        "pop r15",
        "iretq",
        wrapper = sym legacy_dispatch_wrapper,
    )
}

#[unsafe(no_mangle)]
extern "C" fn legacy_dispatch_wrapper(reg_stack: *mut u64) {
    unsafe {
        let ret = crate::syscall::dispatch_fast(
            *reg_stack,
            *reg_stack.add(5),
            *reg_stack.add(4),
            *reg_stack.add(3),
            *reg_stack.add(9),
            *reg_stack.add(7),
        );
        *reg_stack = ret as u64;
    }
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

