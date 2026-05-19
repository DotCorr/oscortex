//! OSCortex /bin/hello — Phase 47 test binary.
//!
//! Prints a greeting and exits. Used to verify the exec-wait syscall round-trip.
#![no_std]
#![no_main]
#![allow(dead_code)]

use core::arch::asm;

core::arch::global_asm!(
    ".section .text._start",
    ".global _start",
    "_start:",
    "   xor  rbp, rbp",
    "   and  rsp, -16",
    "   call hello_main",
    "0: hlt",
    "   jmp  0b",
);

const SYS_WRITE: u64 = 1;
const SYS_EXIT:  u64 = 60;

#[unsafe(no_mangle)]
extern "C" fn hello_main() {
    let msg = b"[hello] Hello from /bin/hello! exec round-trip works.\n";
    unsafe {
        asm!("syscall",
            in("rax") SYS_WRITE,
            in("rdi") 1u64,
            in("rsi") msg.as_ptr() as u64,
            in("rdx") msg.len() as u64,
            lateout("rax") _,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack, preserves_flags));
        asm!("syscall",
            in("rax") SYS_EXIT,
            in("rdi") 42u64,   // exit code 42 — shell prints it
            lateout("rax") _,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack, preserves_flags));
    }
    loop { unsafe { asm!("hlt", options(nostack)); } }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop { unsafe { asm!("hlt", options(nostack)); } }
}
