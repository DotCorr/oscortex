//! Interrupt Descriptor Table — x86_64.
//!
//! All 256 exception/interrupt vectors are wired here.
//! Vectors 0–31: CPU exceptions (handled by kernel / Cortex healing).
//! Vectors 32–47: Legacy IRQ remapping (unused — we use APIC).
//! Vectors 48–127: APIC/MSI interrupts.
//! Vectors 128–191: Cortex inter-core messages.
//! Vector 0x80: syscall software interrupt (legacy path; SYSCALL/SYSRET is primary).
//! Vectors 240–255: kernel-internal IPIs.

use core::arch::asm;
use super::gdt::KERNEL_CS;

pub const DOUBLE_FAULT_IST: u8 = 1;  // IST index for double-fault handler

/// An IDT entry (gate descriptor).
#[derive(Clone, Copy)]
#[repr(C, packed)]
struct IdtEntry {
    offset_lo:  u16,
    selector:   u16,
    ist:        u8,
    type_attr:  u8,
    offset_mid: u16,
    offset_hi:  u32,
    _reserved:  u32,
}

impl IdtEntry {
    const fn missing() -> Self {
        Self {
            offset_lo: 0, selector: 0, ist: 0,
            type_attr: 0, offset_mid: 0, offset_hi: 0, _reserved: 0,
        }
    }

    fn set(&mut self, handler: u64, ist: u8, dpl: u8) {
        self.offset_lo  = handler as u16;
        self.offset_mid = (handler >> 16) as u16;
        self.offset_hi  = (handler >> 32) as u32;
        self.selector   = KERNEL_CS;
        self.ist        = ist & 7;
        // 0x8E = interrupt gate, present, DPL 0
        self.type_attr  = 0x80 | ((dpl & 3) << 5) | 0x0E;
        self._reserved  = 0;
    }
}

#[repr(C, align(16))]
struct Idt {
    entries: [IdtEntry; 256],
}

static mut IDT: Idt = Idt { entries: [IdtEntry::missing(); 256] };

#[repr(C, packed)]
struct IdtPointer { limit: u16, base: u64 }

pub fn init() {
    unsafe {
        // CPU exceptions
        IDT.entries[0].set(div_by_zero_handler as *const () as u64, 0, 0);
        IDT.entries[1].set(debug_handler as *const () as u64, 0, 0);
        IDT.entries[2].set(nmi_handler as *const () as u64, 0, 0);
        IDT.entries[3].set(breakpoint_handler as *const () as u64, 0, 3);
        IDT.entries[4].set(overflow_handler as *const () as u64, 0, 0);
        IDT.entries[6].set(invalid_opcode_handler as *const () as u64, 0, 0);
        IDT.entries[7].set(device_not_avail_handler as *const () as u64, 0, 0);
        IDT.entries[8].set(double_fault_handler as *const () as u64, DOUBLE_FAULT_IST, 0);
        IDT.entries[13].set(general_protection_handler as *const () as u64, 0, 0);
        IDT.entries[14].set(page_fault_handler as *const () as u64, 0, 0);
        IDT.entries[16].set(x87_fp_handler as *const () as u64, 0, 0);
        IDT.entries[17].set(alignment_check_handler as *const () as u64, 0, 0);
        IDT.entries[18].set(machine_check_handler as *const () as u64, 0, 0);
        IDT.entries[19].set(simd_fp_handler as *const () as u64, 0, 0);

        // APIC timer (used by scheduler)
        IDT.entries[0x30].set(apic_timer_entry as *const () as u64, 0, 0);
        // APIC spurious
        IDT.entries[0xFF].set(apic_spurious_handler as *const () as u64, 0, 0);

        // Cortex IPC vectors (128–191)
        for i in 128usize..192 {
            IDT.entries[i].set(cortex_ipc_handler as *const () as u64, 0, 0);
        }

        // PS/2 keyboard (IRQ1 → vector 0x21) and mouse (IRQ12 → vector 0x2C)
        IDT.entries[0x21].set(ps2_kbd_irq_handler as *const () as u64, 0, 0);
        IDT.entries[0x2C].set(ps2_mouse_irq_handler as *const () as u64, 0, 0);

        // Legacy syscall vector (ring 3 → ring 0 path)
        IDT.entries[0x80].set(syscall_legacy_handler as *const () as u64, 0, 3);

        load();
    }
}

pub fn load() {
    unsafe {
        let ptr = IdtPointer {
            limit: (core::mem::size_of::<Idt>() - 1) as u16,
            base: (&IDT as *const Idt) as u64,
        };
        asm!("lidt [{ptr}]", ptr = in(reg) &ptr);
    }
}

// ── Handler stubs ─────────────────────────────────────────────────────────────
//
// Each handler saves/restores state and calls into the Rust handler proper.
// The AI Cortex can intercept any exception via cortex::interrupt_hook().

extern "x86-interrupt" fn div_by_zero_handler(frame: InterruptFrame) {
    crate::cortex::interrupt_hook(0, &frame, None);
    panic!("Division by zero at {:#x}", frame.ip);
}
extern "x86-interrupt" fn debug_handler(_frame: InterruptFrame) {}
extern "x86-interrupt" fn nmi_handler(_frame: InterruptFrame) {}
extern "x86-interrupt" fn breakpoint_handler(frame: InterruptFrame) {
    log::info!("[BP] Breakpoint (int3) at {:#x} cs={:#x}", frame.ip, frame.cs);
}
extern "x86-interrupt" fn overflow_handler(_frame: InterruptFrame) {}
extern "x86-interrupt" fn invalid_opcode_handler(frame: InterruptFrame) {
    crate::cortex::interrupt_hook(6, &frame, None);
    panic!("Invalid opcode at {:#x}", frame.ip);
}
extern "x86-interrupt" fn device_not_avail_handler(_frame: InterruptFrame) {}
#[allow(clippy::diverging_sub_expression)]
extern "x86-interrupt" fn double_fault_handler(frame: InterruptFrame, _err: u64) -> ! {
    panic!("Double fault! ip={:#x}", frame.ip);
}
extern "x86-interrupt" fn general_protection_handler(frame: InterruptFrame, err: u64) {
    crate::cortex::interrupt_hook(13, &frame, Some(err));
    let user_mode = (frame.cs & 0x3) == 0x3;
    if user_mode {
        let pid = crate::process::current_pid();
        log::error!(
            "[GPF] SIGSEGV: ip={:#x} err={:#x} pid={} cs={:#x} sp={:#x} ss={:#x} rflags={:#x}",
            frame.ip,
            err,
            pid,
            frame.cs,
            frame.sp,
            frame.ss,
            frame.flags
        );
        crate::syscall::dump_recent_syscalls(24);
        if pid != 0 {
            crate::process::kill(pid).ok();
        }
        crate::process::set_current_pid(0);
        crate::arch::halt_forever();
    }
    panic!("GPF! ip={:#x} err={:#x}", frame.ip, err);
}
extern "x86-interrupt" fn page_fault_handler(frame: InterruptFrame, err: u64) {
    let cr2: u64;
    unsafe { asm!("mov {}, cr2", out(reg) cr2) };
    crate::cortex::interrupt_hook(14, &frame, Some(err));
    
    // Try to resolve the page fault (demand paging, cow, etc.)
    if !crate::cortex::handle_page_fault(cr2, err) {
        // If it's a user-mode fault (err & 0x4), gracefully kill the process instead of panicking.
        let user_mode = (err & 0x4) != 0;
        if user_mode {
            let pid = crate::process::current_pid();
            log::error!(
                "[PageFault] SIGSEGV: addr={:#x} err={:#x} ip={:#x} pid={} cs={:#x} sp={:#x} ss={:#x} rflags={:#x}",
                cr2,
                err,
                frame.ip,
                pid,
                frame.cs,
                frame.sp,
                frame.ss,
                frame.flags
            );
            crate::syscall::dump_recent_syscalls(24);
            if pid != 0 {
                crate::process::kill(pid).ok();
            }
            crate::process::set_current_pid(0);
            crate::arch::halt_forever();
        }
        // Kernel-mode page fault is always fatal.
        panic!("Unresolvable kernel page fault: addr={:#x} err={:#x} ip={:#x}", cr2, err, frame.ip);
    }
}
extern "x86-interrupt" fn x87_fp_handler(_frame: InterruptFrame) {}
extern "x86-interrupt" fn alignment_check_handler(_frame: InterruptFrame, _err: u64) {}
extern "x86-interrupt" fn machine_check_handler(_frame: InterruptFrame) -> ! {
    panic!("Machine check exception");
}
extern "x86-interrupt" fn simd_fp_handler(_frame: InterruptFrame) {}
extern "x86-interrupt" fn apic_spurious_handler(_frame: InterruptFrame) {}
extern "x86-interrupt" fn cortex_ipc_handler(_frame: InterruptFrame) {
    crate::cortex::ipc::handle_ipc_interrupt();
    crate::arch::apic::eoi();
}
extern "x86-interrupt" fn ps2_kbd_irq_handler(_frame: InterruptFrame) {
    crate::drivers::ps2::kbd_irq();
}

extern "x86-interrupt" fn ps2_mouse_irq_handler(_frame: InterruptFrame) {
    crate::drivers::ps2::mouse_irq();
}

extern "x86-interrupt" fn syscall_legacy_handler(_frame: InterruptFrame) {
    crate::syscall::dispatch_legacy();
}

#[repr(C)]
struct TimerTrapFrame {
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    r11: u64,
    r10: u64,
    r9: u64,
    r8: u64,
    rdi: u64,
    rsi: u64,
    rbp: u64,
    rbx: u64,
    rdx: u64,
    rcx: u64,
    rax: u64,
    rip: u64,
    cs: u64,
    rflags: u64,
}

#[unsafe(naked)]
unsafe extern "C" fn apic_timer_entry() {
    core::arch::naked_asm!(
        // Save all GPRs so preemption is transparent to userspace code.
        "push rax",
        "push rcx",
        "push rdx",
        "push rbx",
        "push rbp",
        "push rsi",
        "push rdi",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov rdi, rsp",
        "call {handler}",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rdi",
        "pop rsi",
        "pop rbp",
        "pop rbx",
        "pop rdx",
        "pop rcx",
        "pop rax",
        "iretq",
        handler = sym apic_timer_handler,
    );
}

extern "C" fn apic_timer_handler(frame_ptr: *mut TimerTrapFrame) {
    let frame = unsafe { &mut *frame_ptr };
    let preempted_user = (frame.cs & 0x3) == 0x3;

    // Early userspace bring-up mode: keep timer interrupts lightweight when
    // preempting user code. Full user-preemptive scheduling is intentionally
    // disabled for v0.1 to avoid pathological timer-starvation at a fixed
    // user RIP on QEMU.
    let _ = preempted_user;

    // EOI first so the APIC can generate the next timer interrupt immediately
    // after iret.
    crate::arch::apic::eoi();

    // Do not run the kernel-task scheduler while handling a user-mode timer
    // interrupt. User preemption/switching is handled by the process path
    // above by rewriting the interrupt frame + CR3 directly; invoking
    // sched::tick() here can context-switch kernel stacks from inside a user
    // interrupt frame and corrupt return flow.
    if !preempted_user {
        crate::sched::tick();
    }

    // Phase 33-A: fire compositor + WM heartbeat at the configured vsync rate
    // (TSC-gated at 60 or 120 Hz).  This replaces the idle-loop compositor::tick()
    // with a hardware-timed call so frame delivery is cadence-accurate.
    //
    // v0.1 STABILITY FIX: never run compositor/WM work while a user thread
    // was preempted. These calls are heavy enough on QEMU (~tens to hundreds
    // of ms) that with a 60 Hz periodic LAPIC timer the ISR overruns the
    // tick interval, leaving the next tick already pending the instant we
    // IRET back to user — user mode is starved of all CPU. Compositor/WM
    // work runs only when the kernel itself was preempted (i.e. idle loop
    // or kernel task), which is the safe context for that work anyway.
    if !preempted_user && crate::arch::apic::vsync_due() {
        crate::compositor::tick();
        crate::wm::tick();
    }
}

/// CPU-provided interrupt frame (pushed by hardware).
#[repr(C)]
pub struct InterruptFrame {
    pub ip:     u64,
    pub cs:     u64,
    pub flags:  u64,
    pub sp:     u64,
    pub ss:     u64,
}

