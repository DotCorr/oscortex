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
        IDT.entries[14].set(page_fault_entry as *const () as u64, 0, 0);
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
    // Kept for any vestigial callers; primary handler is page_fault_entry below.
    let _ = (frame, err);
}

#[repr(C)]
struct PageFaultFrame {
    // Pushed by our naked entry (in reverse pop order).
    r15: u64, r14: u64, r13: u64, r12: u64,
    r11: u64, r10: u64, r9: u64,  r8: u64,
    rdi: u64, rsi: u64, rbp: u64, rbx: u64,
    rdx: u64, rcx: u64, rax: u64,
    // Pushed by the CPU.
    err: u64,
    rip: u64, cs: u64, rflags: u64, rsp: u64, ss: u64,
}

#[unsafe(naked)]
unsafe extern "C" fn page_fault_entry() {
    core::arch::naked_asm!(
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
        // Handler diverges for user faults; for resolved kernel faults it
        // returns and we pop+iretq.
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
        "add rsp, 8",   // discard error code
        "iretq",
        handler = sym page_fault_full_handler,
    );
}

extern "C" fn page_fault_full_handler(frame_ptr: *mut PageFaultFrame) {
    let frame = unsafe { &*frame_ptr };
    let cr2: u64;
    unsafe { asm!("mov {}, cr2", out(reg) cr2) };

    // Build a CPU-pushed-style frame for legacy hooks.
    let legacy = InterruptFrame {
        ip:    frame.rip,
        cs:    frame.cs,
        flags: frame.rflags,
        sp:    frame.rsp,
        ss:    frame.ss,
    };
    crate::cortex::interrupt_hook(14, &legacy, Some(frame.err));

    let resolved = crate::cortex::handle_page_fault(cr2, frame.err);
    if resolved {
        // Demand paging can intentionally resolve user not-present faults.
        // Log a small sample so silent fault loops are visible in serial logs.
        static RESOLVED_PF_LOG: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let n = RESOLVED_PF_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 64 {
            log::warn!(
                "[PageFault-resolved] #{} pid={} addr={:#x} err={:#x} ip={:#x} sp={:#x}",
                n,
                crate::process::current_pid(),
                cr2,
                frame.err,
                frame.rip,
                frame.rsp
            );
        }
        return;
    }

    if !resolved {
        let user_mode = (frame.err & 0x4) != 0;
        if user_mode {
            let pid = crate::process::current_pid();
            log::error!(
                "[PageFault] SIGSEGV: addr={:#x} err={:#x} ip={:#x} pid={} cs={:#x} sp={:#x} ss={:#x} rflags={:#x}",
                cr2, frame.err, frame.rip, pid, frame.cs, frame.rsp, frame.ss, frame.rflags
            );
            log::error!(
                "[PageFault] regs: rax={:#x} rbx={:#x} rcx={:#x} rdx={:#x} rsi={:#x} rdi={:#x} rbp={:#x}",
                frame.rax, frame.rbx, frame.rcx, frame.rdx, frame.rsi, frame.rdi, frame.rbp
            );
            log::error!(
                "[PageFault] regs:  r8={:#x}  r9={:#x} r10={:#x} r11={:#x} r12={:#x} r13={:#x} r14={:#x} r15={:#x}",
                frame.r8, frame.r9, frame.r10, frame.r11, frame.r12, frame.r13, frame.r14, frame.r15
            );
            if frame.rsp != 0 {
                let mut slots = [0u64; 8];
                unsafe {
                    for i in 0..8 {
                        let p = (frame.rsp as *const u64).add(i);
                        slots[i] = core::ptr::read_volatile(p);
                    }
                }
                log::error!(
                    "[PageFault] user stack: [rsp]={:#x} +8={:#x} +16={:#x} +24={:#x} +32={:#x} +40={:#x} +48={:#x} +56={:#x}",
                    slots[0], slots[1], slots[2], slots[3], slots[4], slots[5], slots[6], slots[7]
                );
            }
            // For ip=0 (NULL call), try to disassemble the bytes just before
            // the return-address pushed on the stack — that's the indirect
            // call instruction. Print the 12 bytes preceding [rsp] so we can
            // decode `call *...` and identify the source operand.
            if frame.rip == 0 && frame.rsp != 0 {
                unsafe {
                    let ret_addr = core::ptr::read_volatile(frame.rsp as *const u64);
                    if ret_addr > 0x10 {
                        let start = ret_addr - 12;
                        let mut bytes = [0u8; 12];
                        for i in 0..12 {
                            bytes[i] = core::ptr::read_volatile((start + i as u64) as *const u8);
                        }
                        log::error!(
                            "[PageFault] callsite @ {:#x}-12 bytes: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                            ret_addr,
                            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5],
                            bytes[6], bytes[7], bytes[8], bytes[9], bytes[10], bytes[11]
                        );
                    }
                }
            }
            crate::syscall::dump_recent_syscalls(24);
            if pid != 0 {
                crate::process::kill(pid).ok();
            }
            crate::process::set_current_pid(0);
            crate::arch::halt_forever();
        }
        panic!("Unresolvable kernel page fault: addr={:#x} err={:#x} ip={:#x}", cr2, frame.err, frame.rip);
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
    crate::syscall::check_timerfds_and_wake();

    // Set a deferred-kick flag every ~500 ms (30 ticks × ~17 ms/tick).
    // The flag is consumed in syscall context (sys_pthread_mutex_unlock or
    // sys_wm_event_wait) where spinlock acquisition is safe.  This ISR path
    // only does an atomic store — completely lock-free.
    {
        static TR_KICK_CTR: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let k = TR_KICK_CTR.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if k % 30 == 0 {
            crate::syscall::KICK_REQUESTED.store(
                true,
                core::sync::atomic::Ordering::Release,
            );
        }
    }

    static DUMP_COUNTER: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let count = DUMP_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if count < 10 {
        log::info!("[APIC TIMER] tick count={}", count);
    }
    if count > 0 && count % 500 == 0 {
        crate::process::debug_dump_processes();
    }

    let frame = unsafe { &mut *frame_ptr };
    let preempted_user = (frame.cs & 0x3) == 0x3;

    // EOI first so the APIC can generate the next timer interrupt immediately
    // after iretq.
    crate::arch::apic::eoi();

    // Call wm::tick() and compositor::tick() on vsync boundary even when running userspace threads.
    let vsync = crate::arch::apic::vsync_due();
    if vsync {
        crate::wm::tick();
        crate::compositor::tick();
        crate::drivers::usb::poll();
        crate::arch::apic::reset_vsync_last_tsc();
    }

    if preempted_user {
        // Cooperative `sched_yield` / futex paths handle userspace scheduling.
        // Timer preemption back into a thread that yielded via SYSRET corrupts
        // register state during bring-up (see init supervisor + Flutter host).
        return;
    }

    // Kernel-mode preemption: run cooperative scheduler.
    // Do NOT call sched::tick() while a user thread was running —
    // those are reserved for kernel-idle context only.
    crate::sched::tick();
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

