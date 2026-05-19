//! RISC-V 64 architecture stub.

/// Placeholder interrupt frame — no x86 IDT on RISC-V.
/// Used by cortex subsystem for cross-arch compatibility.
/// `ip` mirrors x86_64 InterruptFrame field name for generic kernel code.
#[derive(Debug)]
#[repr(C)]
pub struct InterruptFrame {
    pub ip:      u64,  // Instruction pointer (= SEPC)
    pub scause:  u64,  // Supervisor Cause Register
    pub stval:   u64,  // Supervisor Trap Value
    pub sstatus: u64,  // Supervisor Status Register
}

/// No-op stub — context switching not yet implemented on riscv64.
pub unsafe extern "C" fn context_switch(_old_sp: *mut u64, _new_sp: u64) {}
/// No-op stub — task entry trampoline not yet implemented on riscv64.
pub unsafe extern "C" fn task_entry() { loop { core::hint::spin_loop() } }

pub fn early_init() { todo!("riscv64 early_init") }
pub fn ap_init() {}
pub fn smp_init(_resp: Option<&'static limine::request::MpResponse>) { /* stub */ }
pub fn halt() { loop { core::hint::spin_loop() } }
pub fn halt_forever() -> ! { loop {} }
pub fn enable_interrupts() {}
pub fn disable_interrupts() {}
pub fn rdtsc() -> u64 {
    let cnt: u64;
    unsafe { core::arch::asm!("csrr {}, time", out(reg) cnt) };
    cnt
}
