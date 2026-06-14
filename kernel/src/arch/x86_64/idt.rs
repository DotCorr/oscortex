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
        // APIC reschedule IPI
        IDT.entries[0x40].set(apic_resched_entry as *const () as u64, 0, 0);
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
        IDT.entries[0x80].set(super::syscall::legacy_syscall_entry as *const () as u64, 0, 3);

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
        if n < 2048 {
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
        let pid = crate::process::current_pid();
        let pml4_phys = crate::process::get_user_context(pid).map(|ctx| ctx.pml4_phys).unwrap_or(0);
        let trans = if pml4_phys != 0 {
            crate::mm::paging::translate_user_page_flags(pml4_phys, cr2)
        } else {
            None
        };
        log::error!(
            "[PageFault] {}: addr={:#x} err={:#x} ip={:#x} pid={} cs={:#x} sp={:#x} ss={:#x} rflags={:#x} pml4={:#x} trans={:?}",
            if user_mode { "SIGSEGV" } else { "KERNEL SEGV" },
            cr2, frame.err, frame.rip, pid, frame.cs, frame.rsp, frame.ss, frame.rflags, pml4_phys, trans
        );
        if pml4_phys != 0 {
            let pml4_idx = ((cr2 >> 39) & 0x1ff) as usize;
            let pdpt_idx = ((cr2 >> 30) & 0x1ff) as usize;
            let pd_idx   = ((cr2 >> 21) & 0x1ff) as usize;
            let pt_idx   = ((cr2 >> 12) & 0x1ff) as usize;
            let phys_to_virt = |phys: u64| -> u64 {
                phys.wrapping_add(crate::mm::frame_allocator::hhdm_offset())
            };
            unsafe {
                let pml4 = phys_to_virt(pml4_phys) as *const u64;
                let pml4e = pml4.add(pml4_idx).read_volatile();
                log::error!("[PageTableWalk] addr={:#x} pml4_idx={} pml4e={:#x}", cr2, pml4_idx, pml4e);
                if pml4e & 1 != 0 {
                    let pdpt = phys_to_virt(pml4e & 0x000f_ffff_ffff_f000) as *const u64;
                    let pdpte = pdpt.add(pdpt_idx).read_volatile();
                    log::error!("[PageTableWalk] pdpt_idx={} pdpte={:#x}", pdpt_idx, pdpte);
                    if pdpte & 1 != 0 {
                        let pd = phys_to_virt(pdpte & 0x000f_ffff_ffff_f000) as *const u64;
                        let pde = pd.add(pd_idx).read_volatile();
                        log::error!("[PageTableWalk] pd_idx={} pde={:#x}", pd_idx, pde);
                        if pde & 1 != 0 {
                            let pt = phys_to_virt(pde & 0x000f_ffff_ffff_f000) as *const u64;
                            let pte = pt.add(pt_idx).read_volatile();
                            log::error!("[PageTableWalk] pt_idx={} pte={:#x}", pt_idx, pte);
                        }
                    }
                }
            }
        }
        log::error!(
            "[PageFault] regs: rax={:#x} rbx={:#x} rcx={:#x} rdx={:#x} rsi={:#x} rdi={:#x} rbp={:#x}",
            frame.rax, frame.rbx, frame.rcx, frame.rdx, frame.rsi, frame.rdi, frame.rbp
        );
        log::error!(
            "[PageFault] regs:  r8={:#x}  r9={:#x} r10={:#x} r11={:#x} r12={:#x} r13={:#x} r14={:#x} r15={:#x}",
            frame.r8, frame.r9, frame.r10, frame.r11, frame.r12, frame.r13, frame.r14, frame.r15
        );
        if frame.rsp != 0 {
            let mut slots = [0u64; 32];
            unsafe {
                for i in 0..32 {
                    let p = (frame.rsp as *const u64).add(i);
                    slots[i] = core::ptr::read_volatile(p);
                }
            }
            log::error!(
                "[PageFault] stack: [rsp]={:#x} +8={:#x} +16={:#x} +24={:#x} +32={:#x} +40={:#x} +48={:#x} +56={:#x} +64={:#x} +72={:#x} +80={:#x} +88={:#x} +96={:#x} +104={:#x} +112={:#x} +120={:#x} +128={:#x} +136={:#x} +144={:#x} +152={:#x} +160={:#x} +168={:#x} +176={:#x} +184={:#x} +192={:#x} +200={:#x} +208={:#x} +216={:#x} +224={:#x} +232={:#x} +240={:#x} +248={:#x}",
                slots[0], slots[1], slots[2], slots[3], slots[4], slots[5], slots[6], slots[7],
                slots[8], slots[9], slots[10], slots[11], slots[12], slots[13], slots[14], slots[15],
                slots[16], slots[17], slots[18], slots[19], slots[20], slots[21], slots[22], slots[23],
                slots[24], slots[25], slots[26], slots[27], slots[28], slots[29], slots[30], slots[31]
            );
        }
        // Virtual-call diagnostic: when executing in the anon/heap window (a bad
        // C++ vtable dispatch), dump r12 (the `this` object), its vtable pointer
        // [r12], and the first vtable slots so we can tell a relocation bug
        // (vtable ptr is engine .text/.data) from object corruption (vtable ptr
        // points into the heap).
        if frame.rip >= 0x3_0000_0000 && frame.rip < 0x100_0000_0000 {
            let rd = |va: u64| -> Option<u64> {
                if va < 0x1000 || va >= 0x0000_8000_0000_0000 { return None; }
                if crate::mm::paging::translate_user_page(pml4_phys, va & !0xFFF).is_none() { return None; }
                Some(unsafe { core::ptr::read_volatile(va as *const u64) })
            };
            if let Some(vptr) = rd(frame.r12) {
                log::error!("[PageFault] vcall: r12={:#x} [r12]=vtable={:#x}", frame.r12, vptr);
                if let (Some(v0), Some(v1), Some(v2)) = (rd(vptr), rd(vptr.wrapping_add(8)), rd(vptr.wrapping_add(16))) {
                    log::error!("[PageFault] vtable: [0]={:#x} [+8]={:#x} [+16]={:#x}", v0, v1, v2);
                    // Dump the runtime bytes at the call target ([vtable+0x10] = v2): if these
                    // are NOT a normal function prologue, the engine .text was corrupted by a
                    // wild write during deserialization.
                    let tgt = v2 & !1;
                    if crate::mm::paging::translate_user_page(pml4_phys, tgt & !0xFFF).is_some() {
                        let mut tb = [0u8; 16];
                        unsafe { for i in 0..16 { tb[i] = core::ptr::read_volatile((tgt + i as u64) as *const u8); } }
                        log::error!(
                            "[PageFault] target-bytes @ {:#x}: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                            tgt, tb[0],tb[1],tb[2],tb[3],tb[4],tb[5],tb[6],tb[7],tb[8],tb[9],tb[10],tb[11],tb[12],tb[13],tb[14],tb[15]
                        );
                    }
                }
            }
            // Bytes at the faulting IP (the executed "stub") — distinguish valid
            // Dart code from garbage/data executed as code.
            if crate::mm::paging::translate_user_page(pml4_phys, frame.rip & !0xFFF).is_some() {
                let mut b = [0u8; 16];
                unsafe { for i in 0..16 { b[i] = core::ptr::read_volatile((frame.rip + i as u64) as *const u8); } }
                log::error!(
                    "[PageFault] ip-bytes @ {:#x}: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                    frame.rip, b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7],b[8],b[9],b[10],b[11],b[12],b[13],b[14],b[15]
                );
            }
        }
        // For ip=0 (NULL call), try to disassemble the bytes just before
        // the return-address pushed on the stack — that's the indirect
        // call instruction. Print the 12 bytes preceding [rsp] so we can
        // decode `call *...` and identify the source operand.
        if (frame.rip == 0 || (frame.rip >= 0x3_0000_0000 && frame.rip < 0x100_0000_0000)) && frame.rsp != 0 {
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
        if user_mode {
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
    user_rsp: u64,
    user_ss: u64,
}

#[unsafe(naked)]
unsafe extern "C" fn apic_resched_entry() {
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
        handler = sym apic_resched_handler,
    );
}

extern "C" fn apic_resched_handler(frame_ptr: *mut TimerTrapFrame) {
    let frame = unsafe { &mut *frame_ptr };
    let preempted_user = (frame.cs & 0x3) == 0x3;

    // EOI first.
    crate::arch::apic::eoi();

    if preempted_user {
        let cur = crate::process::current_pid();
        if cur != 0 {
            let cur_regs = crate::process::UserRegs {
                rip: frame.rip,
                rsp: frame.user_rsp,
                rflags: frame.rflags,
                rax: frame.rax,
                rbx: frame.rbx,
                rcx: frame.rcx,
                rdx: frame.rdx,
                rsi: frame.rsi,
                rdi: frame.rdi,
                rbp: frame.rbp,
                r8: frame.r8,
                r9: frame.r9,
                r10: frame.r10,
                r11: frame.r11,
                r12: frame.r12,
                r13: frame.r13,
                r14: frame.r14,
                r15: frame.r15,
            };
            if let Some((next_pid, next_regs, pml4_phys)) = crate::process::timer_preempt_switch(cur, &cur_regs) {
                unsafe {
                    super::memory::write_cr3(pml4_phys);
                }
                frame.r15 = next_regs.r15;
                frame.r14 = next_regs.r14;
                frame.r13 = next_regs.r13;
                frame.r12 = next_regs.r12;
                frame.r11 = next_regs.r11;
                frame.r10 = next_regs.r10;
                frame.r9 = next_regs.r9;
                frame.r8 = next_regs.r8;
                frame.rdi = next_regs.rdi;
                frame.rsi = next_regs.rsi;
                frame.rbp = next_regs.rbp;
                frame.rbx = next_regs.rbx;
                frame.rdx = next_regs.rdx;
                frame.rcx = next_regs.rcx;
                frame.rax = next_regs.rax;
                frame.rip = next_regs.rip;
                frame.rflags = next_regs.rflags | 0x200;
                frame.user_rsp = next_regs.rsp;
            }
        }
    }
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
    crate::syscall::check_timerfds_and_wake_try();
    crate::process::handle_pending_wakes_try();

    let cpu_id = crate::arch::x86_64::smp::this_cpu().cpu_id;

    // Set a deferred-kick flag every ~500 ms (500 ticks × 1 ms/tick).
    // The flag is consumed in syscall context (sys_pthread_mutex_unlock or
    // sys_wm_event_wait) where spinlock acquisition is safe.  This ISR path
    // only does an atomic store — completely lock-free.
    if cpu_id == 0 {
        static TR_KICK_CTR: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let k = TR_KICK_CTR.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if k % 500 == 0 {
            crate::syscall::KICK_REQUESTED.store(
                true,
                core::sync::atomic::Ordering::Release,
            );
        }
    }


    let frame = unsafe { &mut *frame_ptr };
    let preempted_user = (frame.cs & 0x3) == 0x3;

    // EOI first so the APIC can generate the next timer interrupt immediately
    // after iretq.
    crate::arch::apic::eoi();

    if cpu_id == 0 {
        // Call wm::tick() and compositor::tick() on vsync boundary even when running userspace threads.
        let vsync = crate::arch::apic::vsync_due();
        if vsync {
            crate::wm::tick();
            crate::compositor::tick();
            crate::drivers::usb::poll();
            crate::arch::apic::reset_vsync_last_tsc();
        }
    }

    if preempted_user {
        let cur = crate::process::current_pid();
        let slice_expired = crate::process::account_tick_try(cur).unwrap_or(false);

        let focus = crate::wm::focus_pid();
        let target = if focus != 0 { focus } else { 1 };

        let should_preempt = slice_expired || (
            cur != 0
            && target != 0
            && cur != target
            && !crate::wm::flutter_bootstrap_spin_active()
            && (crate::wm::input_pending_for(target) > 0 || crate::wm::embedder_baton_due(target))
        );

        if should_preempt {
            let cur_regs = crate::process::UserRegs {
                rip: frame.rip,
                rsp: frame.user_rsp,
                rflags: frame.rflags,
                rax: frame.rax,
                rbx: frame.rbx,
                rcx: frame.rcx,
                rdx: frame.rdx,
                rsi: frame.rsi,
                rdi: frame.rdi,
                rbp: frame.rbp,
                r8: frame.r8,
                r9: frame.r9,
                r10: frame.r10,
                r11: frame.r11,
                r12: frame.r12,
                r13: frame.r13,
                r14: frame.r14,
                r15: frame.r15,
            };
            if let Some((next_pid, next_regs, pml4_phys)) = crate::process::timer_preempt_switch_try(cur, &cur_regs) {
                unsafe {
                    super::memory::write_cr3(pml4_phys);
                }

                frame.r15 = next_regs.r15;
                frame.r14 = next_regs.r14;
                frame.r13 = next_regs.r13;
                frame.r12 = next_regs.r12;
                frame.r11 = next_regs.r11;
                frame.r10 = next_regs.r10;
                frame.r9 = next_regs.r9;
                frame.r8 = next_regs.r8;
                frame.rdi = next_regs.rdi;
                frame.rsi = next_regs.rsi;
                frame.rbp = next_regs.rbp;
                frame.rbx = next_regs.rbx;
                frame.rdx = next_regs.rdx;
                frame.rcx = next_regs.rcx;
                frame.rax = next_regs.rax;
                frame.rip = next_regs.rip;
                frame.rflags = next_regs.rflags | 0x200;
                frame.user_rsp = next_regs.rsp;
            }
        }
        return;
    }

    // SMP SAFETY: the scheduler is cooperative and BSP-only — it assumes at most
    // ONE user thread runs at a time. Only CPU 0 may enter/run user threads. If an
    // AP's timer ISR were allowed past here it would claim and enter a user thread
    // (via the wake-assist below), running it CONCURRENTLY with the BSP. Two user
    // threads executing in true parallel break the futex/condvar emulation's
    // single-core assumptions → nondeterministic livelock (the smp>1 failure).
    // Keep APs out: they EOI'd above and now simply return to halt.
    if cpu_id != 0 {
        return;
    }

    // Kernel-mode wake assist: when a Flutter runner is sleeping in a syscall
    // hlt loop, queued input can otherwise leave PID 1 runnable-but-unentered.
    // If the interrupt lands in idle (cur=0), PID 1 is still the safe target
    // for post-init WM input. If it lands in PID 1's own wm_event_wait loop,
    // re-entering PID 1 makes it re-execute the syscall and drain the queue.
    let cur = crate::process::current_pid();
    let focus = crate::wm::focus_pid();
    let target = if focus != 0 { focus } else { 1 };
    if target != 0
        && crate::wm::flutter_init_ready()
        && !crate::wm::flutter_bootstrap_spin_active()
        && (cur == 0 || crate::process::is_blocked_try(cur))
        && (crate::wm::input_pending_for(target) > 0 || crate::wm::embedder_baton_due(target))
    {
        if crate::process::set_state_try(target, crate::process::ProcState::Running) {

            if cur != target {
                let my_cpu = crate::arch::smp::this_cpu().cpu_id;
                if crate::process::try_claim_cpu_for_try(target, my_cpu) {
                    crate::process::enter_user_by_pid_noreturn_try(target);
                }
            }
        }
    }

    // Kernel-mode preemption: run cooperative scheduler.
    // Do NOT call sched::tick() while a user thread was running —
    // those are reserved for kernel-idle context only.
    if crate::arch::x86_64::smp::this_cpu().cpu_id == 0 && crate::process::current_pid() == 0 {
        crate::sched::tick();
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
