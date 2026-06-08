//! aarch64 bring-up milestones 6+7: SVC syscall entry + EL0 user process.
//!
//! This stage proves the kernel can drop to EL0, run a userspace program, and
//! service its `svc #0` syscalls — the headline success bar for the ARM port.
//!
//! We build a tiny self-contained EL0 program (hand-assembled AArch64 machine
//! code), map it + a stack into an EL0-accessible page, install an SVC handler
//! that captures the user GPRs and dispatches the syscall, then `eret` into the
//! program. The program issues several `write` syscalls and an `exit`; the SVC
//! handler prints the writes over serial and stops the machine on exit.
//!
//! Syscall ABI (Linux aarch64 subset): nr in x8, args x0..x5, return in x0.
//!   nr 64  = write(fd, buf, len)   → buf/len printed to the serial console
//!   nr 93  = exit(code)            → halts the bring-up with the code
//!   nr 172 = getpid()             → returns a fixed pid (proves a returning call)

use crate::arch::aarch64::uart;
use crate::arch::aarch64::uart::a64println;
use crate::arch::aarch64::{enter_user, mmu, vectors};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// User VA the program + stack are mapped at (L1 index 16 = 16 GiB, free of the
/// kernel RAM identity map which covers indices 1..2).
const USER_VA_BASE: u64 = 0x4_0000_0000;
/// Offset of the user stack top within the mapped page (page is 4 KiB; code
/// lives at the bottom, stack grows down from near the top).
const USER_STACK_OFF: u64 = 0x0F00;

/// Physical backing page for the EL0 program (BSS, identity-mapped so the
/// kernel can write code into it). 4 KiB, page-aligned.
#[repr(C, align(4096))]
struct UserPage([u8; 4096]);
static mut USER_PAGE: UserPage = UserPage([0; 4096]);

/// How many `write` syscalls the EL0 program issued.
static WRITES: AtomicU64 = AtomicU64::new(0);
/// Set true when the program calls exit.
static EXITED: AtomicBool = AtomicBool::new(false);
static EXIT_CODE: AtomicU64 = AtomicU64::new(0);

/// SVC handler: read the syscall nr/args from the saved user frame, service it,
/// write the return value back into x0. Runs at EL1 with the user frame on the
/// kernel stack; returning lets the vector stub restore + eret to EL0.
fn svc_handler(f: &mut vectors::TrapFrame) {
    // Capture the trapping thread's user registers into the per-CPU snapshot,
    // exactly as the shared dispatch path expects (user_rip/user_rsp/user_gprs).
    crate::arch::aarch64::syscall::capture_from_trap(f);

    // On the first syscall, prove the shared per-CPU snapshot accessors now
    // reflect the trapping EL0 thread (user_rip should be just past the svc,
    // user_rsp should be the EL0 stack).
    static SNAP_CHECKED: AtomicBool = AtomicBool::new(false);
    if !SNAP_CHECKED.swap(true, Ordering::Relaxed) {
        let rip = crate::arch::aarch64::syscall::user_rip();
        let rsp = crate::arch::aarch64::syscall::user_rsp();
        uart::puts("[svc] snapshot: user_rip=");
        uart::puthex_full(rip);
        uart::puts(" user_rsp=");
        uart::puthex_full(rsp);
        uart::puts("\n");
    }

    let nr = f.x[8];
    match nr {
        64 => {
            // write(fd=x0, buf=x1, len=x2). buf is a user VA; since the user
            // page is also identity-mapped via its physical address we read it
            // through the physical alias for safety.
            let buf_va = f.x[1];
            let len = f.x[2] as usize;
            print_user_buf(buf_va, len);
            WRITES.fetch_add(1, Ordering::Relaxed);
            f.x[0] = len as u64; // bytes written
        }
        172 => {
            // getpid() — return a fixed value to prove a value round-trips.
            f.x[0] = 0x05;
        }
        93 => {
            EXIT_CODE.store(f.x[0], Ordering::Release);
            EXITED.store(true, Ordering::Release);
            // Leave the frame; the post-eret instruction is a spin we never
            // re-enter because the bring-up loop below sees EXITED and stops.
            f.x[0] = 0;
        }
        _ => {
            uart::puts("[svc] unknown nr=");
            uart::puthex(nr);
            uart::puts("\n");
            f.x[0] = u64::MAX; // -1
        }
    }
}

/// Print `len` bytes from user VA `buf_va`. The user VA maps to a known offset
/// inside USER_PAGE; translate via that physical alias.
fn print_user_buf(buf_va: u64, len: usize) {
    let page_base = unsafe { core::ptr::addr_of!(USER_PAGE) as u64 };
    // user VA = USER_VA_BASE + offset → physical = page_base + offset.
    if buf_va < USER_VA_BASE || buf_va >= USER_VA_BASE + 4096 {
        return;
    }
    let off = (buf_va - USER_VA_BASE) as usize;
    let end = (off + len).min(4096);
    let base = unsafe { core::ptr::addr_of!((*core::ptr::addr_of!(USER_PAGE)).0) as *const u8 };
    for i in off..end {
        let b = unsafe { core::ptr::read_volatile(base.add(i)) };
        uart::putb(b);
    }
    let _ = page_base;
}

/// Assemble the EL0 test program into the user page and return (entry_off,
/// msg_off, msg_len). The program issues two writes, a getpid, a third write,
/// then exit(7).
fn build_user_program() -> (u64, u64, usize) {
    // Message lives at the top half of the page; code at the bottom.
    let msg = b"[EL0] hello from userspace via svc #0\n";
    let msg_off = 0x800usize;
    let pid_msg = b"[EL0] getpid returned a value, calling exit(7)\n";
    let pid_msg_off = 0x880usize;

    unsafe {
        let page = &mut (*core::ptr::addr_of_mut!(USER_PAGE)).0;
        page[msg_off..msg_off + msg.len()].copy_from_slice(msg);
        page[pid_msg_off..pid_msg_off + pid_msg.len()].copy_from_slice(pid_msg);
    }

    // Hand-assembled AArch64 at code offset 0. We compute absolute user VAs for
    // the message buffers (USER_VA_BASE + off) and emit instructions to load
    // them. We keep it simple: a sequence of MOV-immediate + svc.
    let code_off = 0usize;
    let mut asm: heapless_vec::Vec = heapless_vec::Vec::new();

    let msg_va = USER_VA_BASE + msg_off as u64;
    let pid_msg_va = USER_VA_BASE + pid_msg_off as u64;

    // write(1, msg_va, msg.len())
    emit_load_imm(&mut asm, 0, 1); // x0 = 1 (fd)
    emit_load_imm(&mut asm, 1, msg_va); // x1 = buf
    emit_load_imm(&mut asm, 2, msg.len() as u64); // x2 = len
    emit_load_imm(&mut asm, 8, 64); // x8 = write
    asm.push(SVC0);

    // write again (proves repeated syscalls + that x-regs survive)
    emit_load_imm(&mut asm, 0, 1);
    emit_load_imm(&mut asm, 1, msg_va);
    emit_load_imm(&mut asm, 2, msg.len() as u64);
    emit_load_imm(&mut asm, 8, 64);
    asm.push(SVC0);

    // getpid()
    emit_load_imm(&mut asm, 8, 172);
    asm.push(SVC0);

    // write(1, pid_msg_va, pid_msg.len())
    emit_load_imm(&mut asm, 0, 1);
    emit_load_imm(&mut asm, 1, pid_msg_va);
    emit_load_imm(&mut asm, 2, pid_msg.len() as u64);
    emit_load_imm(&mut asm, 8, 64);
    asm.push(SVC0);

    // exit(7)
    emit_load_imm(&mut asm, 0, 7);
    emit_load_imm(&mut asm, 8, 93);
    asm.push(SVC0);

    // Safety net: infinite loop if exit ever returns.
    asm.push(0x14000000); // b .

    // Copy the assembled words into the page at code_off.
    unsafe {
        let page = &mut (*core::ptr::addr_of_mut!(USER_PAGE)).0;
        for (i, w) in asm.iter().enumerate() {
            let b = w.to_le_bytes();
            let o = code_off + i * 4;
            page[o..o + 4].copy_from_slice(&b);
        }
    }

    (code_off as u64, msg_off as u64, msg.len())
}

/// `svc #0` encoding.
const SVC0: u32 = 0xD4000001;

/// Emit a MOVZ/MOVK sequence loading the 64-bit immediate `imm` into x`reg`.
fn emit_load_imm(asm: &mut heapless_vec::Vec, reg: u32, imm: u64) {
    // MOVZ Xd, #imm16, LSL #shift : 0xD2800000 | (hw<<21) | (imm16<<5) | Rd
    // MOVK Xd, #imm16, LSL #shift : 0xF2800000 | (hw<<21) | (imm16<<5) | Rd
    let mut first = true;
    for hw in 0..4u32 {
        let imm16 = ((imm >> (hw * 16)) & 0xFFFF) as u32;
        if imm16 == 0 && !(first && hw == 3) {
            // Skip zero halfwords, but always emit at least one (MOVZ #0).
            if !first {
                continue;
            }
        }
        let base = if first { 0xD280_0000 } else { 0xF280_0000 };
        let insn = base | (hw << 21) | (imm16 << 5) | reg;
        asm.push(insn);
        first = false;
    }
    if first {
        // imm was all zero → emit MOVZ Xreg, #0.
        asm.push(0xD280_0000 | reg);
    }
}

pub fn run() -> ! {
    a64println!("[boot] Milestone 6 (SVC) + 7 (EL0): launching a user process...");

    // Map the user page into EL0 at USER_VA_BASE.
    let pa = unsafe { core::ptr::addr_of!(USER_PAGE) as u64 };
    unsafe { mmu::map_user_page(USER_VA_BASE, pa) };
    uart::puts("[boot] mapped EL0 page: VA ");
    uart::puthex_full(USER_VA_BASE);
    uart::puts(" -> PA ");
    uart::puthex_full(pa);
    uart::puts("\n");

    // Assemble the EL0 program + install the SVC handler.
    let (entry_off, _msg_off, _msg_len) = build_user_program();
    vectors::set_svc_handler(svc_handler);

    let user_pc = USER_VA_BASE + entry_off;
    let user_sp = USER_VA_BASE + USER_STACK_OFF;
    uart::puts("[boot] entering EL0: PC ");
    uart::puthex_full(user_pc);
    uart::puts(" SP ");
    uart::puthex_full(user_sp);
    uart::puts("\n");

    // The SVC handler runs on the EL1 stack and returns to EL0 via the vector
    // stub's eret. exit() sets EXITED; an EL1 IRQ (timer) periodically re-enters
    // the kernel, but to deterministically observe exit we poll a shared flag
    // from a watcher that the timer IRQ can't see — so instead we make exit
    // *not* return to EL0: it leaves the user PC spinning while we detect the
    // flag here. We launch EL0 on a separate path that returns control to this
    // loop once EXITED is set by routing exit through a longjmp-free re-entry.
    //
    // Simplest correct approach: enter EL0; the program runs to exit which sets
    // the flag; the post-exit `b .` spins in EL0; the EL1 timer IRQ fires,
    // re-enters the kernel, and our IRQ-side check below notices EXITED and
    // finalizes. We install that check by wrapping the existing IRQ handler.
    install_exit_watch_irq();

    unsafe { enter_user::enter_el0_at(user_pc, user_sp, 0, 0, 0) }
}

/// Wrap the timer IRQ so that, once the EL0 program has called exit(), the next
/// timer tick finalizes the bring-up from EL1 (the EL0 program is spinning in
/// `b .`). This lets us observe the syscall results and stop cleanly.
fn install_exit_watch_irq() {
    use crate::arch::aarch64::{gic, timer};
    fn watch_irq(_f: &mut vectors::TrapFrame) {
        let intid = gic::acknowledge();
        if intid == timer::TIMER_PPI {
            timer::on_tick();
            if EXITED.load(Ordering::Acquire) {
                gic::eoi(intid);
                finalize();
            }
        }
        if intid < 1020 {
            gic::eoi(intid);
        }
    }
    vectors::set_irq_handler(watch_irq);
}

/// Report the syscall results and stop the machine.
fn finalize() -> ! {
    let writes = WRITES.load(Ordering::Relaxed);
    let code = EXIT_CODE.load(Ordering::Relaxed);
    a64println!();
    a64println!(
        "[boot] EL0 process serviced: writes={} exit_code={}",
        writes, code
    );
    if writes == 3 && code == 7 {
        a64println!("[boot] Milestone 6+7 (SVC + EL0) complete — userspace serviced over svc #0.");
        a64println!();
        a64println!("========================================");
        a64println!("  OSCortex aarch64: BOOT -> EL0 -> SYSCALLS -> EXIT");
        a64println!("  All bring-up milestones passed.");
        a64println!("========================================");
    } else {
        a64println!("[boot] Milestone 6+7 FAILED — unexpected syscall results.");
    }
    // Power off via PSCI SYSTEM_OFF so the test harness exits cleanly.
    crate::arch::aarch64::psci::system_off();
}

// ── Minimal fixed-capacity Vec for assembling the EL0 program (no alloc dep) ──
mod heapless_vec {
    pub struct Vec {
        buf: [u32; 256],
        len: usize,
    }
    impl Vec {
        pub fn new() -> Self {
            Vec { buf: [0; 256], len: 0 }
        }
        pub fn push(&mut self, v: u32) {
            if self.len < self.buf.len() {
                self.buf[self.len] = v;
                self.len += 1;
            }
        }
        pub fn iter(&self) -> core::slice::Iter<'_, u32> {
            self.buf[..self.len].iter()
        }
    }
}
