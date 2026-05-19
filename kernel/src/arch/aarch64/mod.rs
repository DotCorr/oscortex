//! aarch64 architecture stub.

/// Placeholder interrupt frame — no x86 IDT on aarch64.
/// Used by cortex subsystem for cross-arch compatibility.
/// `ip` mirrors x86_64 InterruptFrame field name for generic kernel code.
#[derive(Debug)]
#[repr(C)]
pub struct InterruptFrame {
    pub ip:    u64,  // Instruction pointer (= ELR_EL1)
    pub spsr:  u64,  // Saved Program Status Register
    pub esr:   u64,  // Exception Syndrome Register
    pub far:   u64,  // Fault Address Register
}

/// No-op stub — context switching not yet implemented on aarch64.
pub unsafe extern "C" fn context_switch(_old_sp: *mut u64, _new_sp: u64) {}
/// No-op stub — task entry trampoline not yet implemented on aarch64.
pub unsafe extern "C" fn task_entry() { loop { core::hint::spin_loop() } }

pub fn early_init() { todo!("aarch64 early_init") }
pub fn ap_init() { todo!("aarch64 ap_init") }
pub fn smp_init(_resp: Option<&'static limine::request::MpResponse>) { /* stub */ }
pub fn halt() { loop { core::hint::spin_loop() } }
pub fn halt_forever() -> ! { loop { core::hint::spin_loop() } }
pub fn enable_interrupts() {}
pub fn disable_interrupts() {}
pub fn rdtsc() -> u64 {
    let cnt: u64;
    unsafe { core::arch::asm!("mrs {}, cntvct_el0", out(reg) cnt) };
    cnt
}
