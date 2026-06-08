//! x86_64 architecture implementation.
//!
//! Covers: GDT, IDT, TSS, APIC, MSRs, CPU feature detection, syscall (SYSCALL/SYSRET).

pub mod gdt;
pub mod idt;
pub mod interrupts;
pub mod memory;
pub mod apic;
pub mod cpu;
pub(crate) mod pci;
pub(crate) mod port_io;
pub mod syscall;
pub mod acpi;
pub mod smp;
pub mod enter_user;

pub use enter_user::{enter_user_iret, enter_user_sysret, EnterUserRegs};

// Re-export commonly used types so external code uses `crate::arch::InterruptFrame`
pub use idt::InterruptFrame;

use core::arch::asm;

/// Perform early arch initialisation (BSP path).
///
/// Called before any allocator is available — must use only static storage.
pub fn early_init() {
    // Verify required CPU features.
    crate::logger::early_print("[ARCH] assert_required_features\r\n");
    cpu::assert_required_features();
    // Load the GDT with kernel code/data + TSS for interrupt stacks.
    crate::logger::early_print("[ARCH] gdt::init\r\n");
    gdt::init();
    // Load the IDT with all hardware + software interrupt handlers.
    crate::logger::early_print("[ARCH] idt::init\r\n");
    idt::init();
    // Disable legacy PIC (we use APIC).
    crate::logger::early_print("[ARCH] disable_pic\r\n");
    unsafe { interrupts::disable_pic() };
    // Init local APIC on BSP — x2APIC is MSR-based (no mapping); xAPIC MMIO
    // mapping is deferred to after mm::init() via finish_xapic_init().
    crate::logger::early_print("[ARCH] apic::init_bsp\r\n");
    apic::init_bsp();
    // Enable SSE/AVX for the AI inference engine.
    crate::logger::early_print("[ARCH] cpu::enable_fpu_simd\r\n");
    cpu::enable_fpu_simd();
    // Enable SYSCALL/SYSRET path.
    crate::logger::early_print("[ARCH] syscall::init\r\n");
    syscall::init();
    crate::logger::early_print("[ARCH] early_init complete\r\n");
}

/// Per-AP (Application Processor) init — called after BSP completes kernel_main setup.
pub fn ap_init(cpu_idx: u32) {
    gdt::init_ap(cpu_idx);
    idt::load();
    apic::init_ap();
    cpu::enable_fpu_simd();
    syscall::init_ap(cpu_idx);
}

/// Wake all APs and bring SMP online. Call after heap + logging are ready.
pub fn smp_init(resp: Option<&'static limine::request::MpResponse>) {
    smp::init(resp);
}

/// Halt the current CPU core until an interrupt arrives.
#[inline(always)]
pub fn halt() {
    unsafe { asm!("hlt") }
}

/// Enable interrupts, halt until one arrives, then disable interrupts again.
///
/// The `sti; hlt` pair is atomic on x86 (STI defers IF until after the next
/// instruction), so no interrupt can sneak in between enabling and halting.
#[inline(always)]
pub fn enable_and_halt() {
    unsafe { asm!("sti; hlt; cli", options(nomem, nostack)) }
}

/// Permanently halt (used in panic).
pub fn halt_forever() -> ! {
    loop {
        unsafe { asm!("cli; hlt") }
    }
}

// ── Context switch ────────────────────────────────────────────────────────────

/// Switch from the current task to a new one.
///
/// Saves the six callee-saved registers (rbx, rbp, r12-r15) of the old task
/// onto its kernel stack, stores RSP into `*old_sp`, loads `new_sp` as the new
/// RSP, then restores the new task's saved registers and returns.
///
/// # Safety
/// * `old_sp` must point to the `kernel_sp` field of the currently running
///   [`crate::sched::Task`].
/// * `new_sp` must be a valid kernel stack pointer prepared by
///   [`crate::sched::spawn_kernel_task`] or by a previous call to this
///   function.
/// * Must be called with interrupts disabled (inside ISR or `cortex::run`
///   halt loop before `sti`).
#[unsafe(naked)]
pub unsafe extern "C" fn context_switch(old_sp: *mut u64, new_sp: u64) {
    core::arch::naked_asm!(
        // Save callee-saved registers of the old task.
        "push rbx",
        "push rbp",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        // rdi = old_sp (System V AMD64: first arg), save current RSP.
        "mov [rdi], rsp",
        // rsi = new_sp (second arg), load new RSP.
        "mov rsp, rsi",
        // Restore new task's callee-saved registers.
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbp",
        "pop rbx",
        // Return — jumps to the new task's saved return address.
        "ret",
    )
}

/// Trampoline entered by a freshly spawned task on its very first schedule.
///
/// The task function pointer is the top-of-stack word left there by
/// `spawn_kernel_task`. We pop it into `rdi`, enable interrupts, then call it.
/// If the task function ever returns (it shouldn't for kernel tasks), we park.
#[unsafe(naked)]
pub unsafe extern "C" fn task_entry() {
    core::arch::naked_asm!(
        "pop  rdi",    // task fn pointer pushed by spawn_kernel_task
        "sti",         // enable interrupts for this task
        "call rdi",    // run the task function
        // Task returned — should not happen for kernel tasks; park.
        "0:",
        "cli",
        "hlt",
        "jmp 0b",
    )
}

/// Enable hardware interrupts on this core.
#[inline(always)]
pub fn enable_interrupts() {
    unsafe { asm!("sti") }
}

/// Disable hardware interrupts on this core.
#[inline(always)]
pub fn disable_interrupts() {
    unsafe { asm!("cli") }
}

/// Read the CPU timestamp counter.
#[inline(always)]
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
    }
    ((hi as u64) << 32) | (lo as u64)
}
