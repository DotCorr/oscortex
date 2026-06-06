//! PS/2 keyboard and mouse driver — Phase 37-A.
//!
//! Replaces the soft-polling loop in `wm::tick()` with a real IRQ-driven
//! path so keystrokes and mouse deltas are delivered at interrupt time rather
//! than waiting up to one APIC timer period (~10 ms).
//!
//! ## Hardware overview
//!
//! The Intel 8042 PS/2 controller has two ports:
//!   * Port 1 (keyboard) — IRQ1 on the legacy 8259 master PIC → vector 0x21
//!   * Port 2 (mouse)    — IRQ12 on the legacy 8259 slave PIC  → vector 0x2C
//!
//! Because OSCortex uses x2APIC for the timer, the 8259 is remapped but fully
//! masked at boot.  `enable_pic_irqs()` below unmasks exactly IRQ1 and IRQ12
//! and configures the local APIC's LINT0 pin for ExtINT delivery so PIC IRQs
//! propagate through the APIC to the CPU.
//!
//! ## Interaction with wm::tick() polling
//!
//! `wm::tick()` calls `poll_ps2_input()` which drains any bytes still sitting
//! in the 8042's output buffer.  Once IRQs are live that buffer is consumed
//! promptly in the ISR, so `poll_ps2_input` usually returns immediately.
//! Both paths share `wm::push_key` / `wm::push_pointer` and are therefore
//! safe to run concurrently (spin::Mutex inside those helpers).

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// True once `init()` completed successfully.
pub static PS2_READY: AtomicBool = AtomicBool::new(false);

/// Bit-field of detected devices: bit 0 = keyboard, bit 1 = mouse.
pub static PS2_DEVICES: AtomicU8 = AtomicU8::new(0);

// ── I/O port helpers ─────────────────────────────────────────────────────────

/// 8042 data port.
const PS2_DATA:   u16 = 0x60;
/// 8042 status/command port.
const PS2_STATUS: u16 = 0x64;

/// 8042 status register: bit 0 = output buffer full (data ready to read).
const STATUS_OBF: u8 = 0x01;
/// 8042 status register: bit 5 = data came from mouse (auxiliary device).
const STATUS_MOUSE: u8 = 0x20;

#[inline(always)]
unsafe fn in8(port: u16) -> u8 {
    crate::arch::port_io::inb(port)
}

#[inline(always)]
unsafe fn out8(port: u16, val: u8) {
    crate::arch::port_io::outb(port, val);
}

/// Wait until the 8042 input buffer is empty (ready for a new command).
#[inline]
unsafe fn wait_write() {
    for _ in 0..100_000u32 {
        if unsafe { in8(PS2_STATUS) } & 0x02 == 0 {
            return;
        }
        core::hint::spin_loop();
    }
}

/// Wait until the 8042 output buffer is full (data available).
#[inline]
unsafe fn wait_read() -> bool {
    for _ in 0..100_000u32 {
        if unsafe { in8(PS2_STATUS) } & STATUS_OBF != 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

// ── 8259 PIC helpers ─────────────────────────────────────────────────────────

const PIC_MASTER_CMD:  u16 = 0x20;
const PIC_MASTER_DATA: u16 = 0x21;
const PIC_SLAVE_CMD:   u16 = 0xA0;
const PIC_SLAVE_DATA:  u16 = 0xA1;
/// Non-specific EOI command.
const PIC_EOI: u8 = 0x20;

/// Unmask IRQ1 (keyboard) on master PIC and IRQ12 (mouse, slave IRQ4)
/// on the slave PIC.  Cascade IRQ2 on the master is also unmasked so the
/// slave PIC's output reaches the master.
///
/// Assumes the PIC has already been reinitialised by `disable_pic()` with
/// master remapped to vectors 0x20-0x27, slave to 0x28-0x2F.
///
/// # Safety
/// Must be called after IDT vectors 0x21 and 0x2C are registered.
pub unsafe fn enable_pic_irqs() {
    // Master: unmask IRQ1 (bit 1) and IRQ2/cascade (bit 2); keep rest masked.
    // 0b_1111_1001 = 0xF9 masks all except IRQ1 and IRQ2.
    out8(PIC_MASTER_DATA, 0xF9);
    // Slave: unmask IRQ12 = slave IRQ4 (bit 4); keep rest masked.
    // 0b_1110_1111 = 0xEF masks all except slave IRQ4.
    out8(PIC_SLAVE_DATA, 0xEF);

    // Configure local APIC LINT0 for ExtINT delivery so PIC IRQs arrive.
    // Delegates to apic module which handles both x2APIC (MSR) and xAPIC (MMIO)
    // and is a safe no-op if neither APIC mode is active.
    crate::arch::apic::configure_lint0_for_extint();
}

/// Send EOI to the master PIC.
#[inline(always)]
pub unsafe fn pic_eoi_master() {
    out8(PIC_MASTER_CMD, PIC_EOI);
}

/// Send EOI to both slave and master PICs (needed for IRQ8-15).
#[inline(always)]
pub unsafe fn pic_eoi_slave() {
    out8(PIC_SLAVE_CMD, PIC_EOI);
    out8(PIC_MASTER_CMD, PIC_EOI);
}

// ── Mouse accumulator ─────────────────────────────────────────────────────────

/// 3-byte PS/2 mouse packet accumulator.  Index 0 = flags byte.
static MOUSE_BUF: [AtomicU8; 3] = [
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
];
/// How many bytes of the current packet have been received (0..3).
static MOUSE_IDX: AtomicU8 = AtomicU8::new(0);

/// Absolute cursor position, clamped to framebuffer dimensions.
static CURSOR_X: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(32);
static CURSOR_Y: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(32);

/// Current mouse button click state.
static CURSOR_BUTTONS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// TSC cycle of the last mouse activity (movement or click).
static LAST_ACTIVITY_TSC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

// ── E0-prefix tracking for extended keyboard scancodes ───────────────────────

static KBD_E0: AtomicBool = AtomicBool::new(false);

// ── Public IRQ entry points ───────────────────────────────────────────────────

/// Return the current absolute cursor position (x, y) in screen pixels.
pub fn cursor_pos() -> (i32, i32) {
    (
        CURSOR_X.load(Ordering::Relaxed),
        CURSOR_Y.load(Ordering::Relaxed),
    )
}

/// Return the current mouse button state.
pub fn cursor_buttons() -> u32 {
    CURSOR_BUTTONS.load(Ordering::Relaxed)
}

/// Return the TSC cycle of the last mouse activity.
pub fn last_activity_tsc() -> u64 {
    LAST_ACTIVITY_TSC.load(Ordering::Relaxed)
}

/// Update cursor position (poll path + IRQ path share the same atomics).
pub fn set_cursor_pos(x: i32, y: i32) {
    CURSOR_X.store(x, Ordering::Relaxed);
    CURSOR_Y.store(y, Ordering::Relaxed);
    LAST_ACTIVITY_TSC.store(crate::arch::rdtsc(), Ordering::Relaxed);
}

/// Update cursor button state.
pub fn set_cursor_buttons(buttons: u32) {
    CURSOR_BUTTONS.store(buttons, Ordering::Relaxed);
    LAST_ACTIVITY_TSC.store(crate::arch::rdtsc(), Ordering::Relaxed);
}

/// Called from IDT vector 0x21 (PS/2 keyboard IRQ1).
/// Reads one byte from the 8042 and dispatches it to the WM event queue.
pub fn kbd_irq() {
    // Read the scancode — do not check OBF here; we were interrupted because
    // OBF became set, so we must read to clear the interrupt.
    let byte = unsafe { in8(PS2_DATA) };
    let status = unsafe { in8(PS2_STATUS) };

    log::trace!("[PS2 KBD] byte=0x{:02X} status=0x{:02X}", byte, status);

    // If the byte is actually from the mouse (auxiliary) port, re-route.
    if status & STATUS_MOUSE != 0 {
        handle_mouse_byte(byte);
        unsafe { pic_eoi_slave(); }
        return;
    }

    if byte == 0xE0 {
        KBD_E0.store(true, Ordering::Release);
        unsafe { pic_eoi_master(); }
        return;
    }

    let pressed = (byte & 0x80) == 0;
    let sc      = (byte & 0x7F) as u32;
    let scancode = if KBD_E0.swap(false, Ordering::AcqRel) {
        0xE000 | sc
    } else {
        sc
    };

    // Rate-limited log for debugging keypresses
    static KBD_SAMPLE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    if KBD_SAMPLE.fetch_add(1, Ordering::Relaxed) < 32 {
        log::warn!("[PS2 KBD Dispatch] scancode={:#x} ({}) pressed={}", scancode, scancode, pressed);
    }

    crate::wm::push_key(scancode, pressed);
    unsafe { pic_eoi_master(); }
}

/// Called from IDT vector 0x2C (PS/2 mouse IRQ12).
/// Accumulates 3-byte packets and pushes pointer events.
pub fn mouse_irq() {
    let byte = unsafe { in8(PS2_DATA) };
    static MOUSE_IRQ_LOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    if MOUSE_IRQ_LOG.fetch_add(1, Ordering::Relaxed) < 16 {
        log::warn!("[PS2 MOUSE IRQ12] byte=0x{:02X}", byte);
    }
    handle_mouse_byte(byte);
    unsafe { pic_eoi_slave(); }
}

fn handle_mouse_byte(byte: u8) {
    let idx = MOUSE_IDX.load(Ordering::Relaxed);

    // Byte-0 (flags) resync gate. A real first packet byte ALWAYS has bit3=1
    // (always-one). It must ALSO not be a controller response byte — ACK (0xFA),
    // RESEND (0xFE) and NAK/ERROR (0xFC) all have bits 6&7 set (0xC0). Those bytes
    // get injected into the data stream (e.g. the ACK to the 0xF4 enable command,
    // or any later command) and, if absorbed as a flags byte, permanently
    // MISALIGN the 3-byte accumulator: the misaligned flags then carry overflow
    // bits and every packet is discarded → the cursor freezes (only the rare
    // lucky resync gets through). A normal MOVEMENT flags byte effectively never
    // has both overflow bits set, so rejecting 0xC0 here cleanly skips the
    // spurious byte and resyncs on the next true flags byte. (Genuine
    // double-overflow movement packets are discarded below anyway.)
    if idx == 0 && ((byte & 0x08) == 0 || (byte & 0xC0) != 0) {
        return; // not a valid packet start (desync / ACK / resend) — skip & resync
    }

    MOUSE_BUF[idx as usize].store(byte, Ordering::Relaxed);
    let next_idx = idx + 1;

    if next_idx < 3 {
        MOUSE_IDX.store(next_idx, Ordering::Release);
        return;
    }

    // Full 3-byte packet ready.
    MOUSE_IDX.store(0, Ordering::Release);

    let p0 = MOUSE_BUF[0].load(Ordering::Relaxed);
    let p1 = MOUSE_BUF[1].load(Ordering::Relaxed);
    let p2 = MOUSE_BUF[2].load(Ordering::Relaxed);

    // Overflow packets are unreliable — discard.
    if (p0 & 0xC0) != 0 { return; }

    // Sign-extend delta bytes using the sign bits in the flags byte.
    let raw_dx: i32 = if p0 & 0x10 != 0 { (p1 as i8) as i32 } else { p1 as i32 };
    let raw_dy: i32 = if p0 & 0x20 != 0 { (p2 as i8) as i32 } else { p2 as i32 };

    // Lightweight acceleration curve tuned for QEMU PS/2 packets.
    // Small deltas stay precise; medium/large movement gets amplified.
    let accel = |d: i32| -> i32 {
        let a = d.abs();
        let gain = if a >= 8 {
            3
        } else if a >= 4 {
            2
        } else {
            1
        };
        d.saturating_mul(gain)
    };
    let dx = accel(raw_dx);
    let dy = accel(raw_dy);

    // Get framebuffer bounds.
    let (max_w, max_h) = crate::drivers::fb::size_px()
        .map(|(w, h)| (w as i32, h as i32))
        .unwrap_or((640, 480));

    let mut x = 32;
    let _ = CURSOR_X.fetch_update(Ordering::AcqRel, Ordering::Relaxed, |v| {
        let nv = (v + dx).clamp(0, (max_w - 1).max(0));
        x = nv;
        Some(nv)
    });
    // PS/2 Y is positive = move up; screen Y is positive = move down → negate dy.
    let mut y = 32;
    let _ = CURSOR_Y.fetch_update(Ordering::AcqRel, Ordering::Relaxed, |v| {
        let nv = (v - dy).clamp(0, (max_h - 1).max(0));
        y = nv;
        Some(nv)
    });

    let buttons = (p0 & 0x07) as u32;
    CURSOR_BUTTONS.store(buttons, Ordering::Relaxed);
    LAST_ACTIVITY_TSC.store(crate::arch::rdtsc(), Ordering::Relaxed);
    crate::compositor::invalidate();
    static PTR_LOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    if PTR_LOG.fetch_add(1, Ordering::Relaxed) < 16 {
        log::warn!("[PS2 MOUSE] push_pointer x={} y={} buttons={}", x, y, buttons);
    }
    crate::wm::push_pointer(x, y, buttons);
}

// ── Initialisation ────────────────────────────────────────────────────────────

/// Detect and initialise the PS/2 controller and its connected devices.
///
/// Steps:
///   1. Flush the 8042 output buffer.
///   2. Disable both PS/2 ports (commands 0xAD and 0xA7).
///   3. Read + patch the controller command byte: enable IRQs, disable scan
///      code translation.
///   4. Enable both ports (commands 0xAE and 0xA8).
///   5. Activate the mouse by sending the Enable Data Reporting command.
///   6. Mark `PS2_READY` true.
///
/// `enable_pic_irqs()` is **not** called here — it must be called after the
/// IDT vectors are registered (i.e., after `idt::init()`).
pub fn init() {
    unsafe {
        // Flush output buffer.
        while in8(PS2_STATUS) & STATUS_OBF != 0 {
            let _ = in8(PS2_DATA);
        }

        // Disable both ports so no interference during setup.
        wait_write(); out8(PS2_STATUS, 0xAD); // disable port 1
        wait_write(); out8(PS2_STATUS, 0xA7); // disable port 2

        // Flush again after disable.
        while in8(PS2_STATUS) & STATUS_OBF != 0 {
            let _ = in8(PS2_DATA);
        }

        // Read current command byte.
        wait_write(); out8(PS2_STATUS, 0x20); // "read command byte" command
        let cmd_byte = if wait_read() { in8(PS2_DATA) } else { 0x47 };

        // Patch: enable port-1 IRQ (bit 0), enable port-2 IRQ (bit 1),
        // enable scan-code translation (set bit 6).
        let cmd_byte = cmd_byte | 0x43;

        // Write patched command byte back.
        wait_write(); out8(PS2_STATUS, 0x60);
        wait_write(); out8(PS2_DATA, cmd_byte);

        // Re-enable both ports.
        wait_write(); out8(PS2_STATUS, 0xAE); // enable port 1 (keyboard)
        wait_write(); out8(PS2_STATUS, 0xA8); // enable port 2 (mouse)

        // Activate mouse data reporting: "send byte to auxiliary" (0xD4) then 0xF4.
        wait_write(); out8(PS2_STATUS, 0xD4);
        wait_write(); out8(PS2_DATA, 0xF4);

        // Flush ACK byte from mouse.
        let _ack = if wait_read() { in8(PS2_DATA) } else { 0 };

        let mut devs: u8 = 0x01; // keyboard always assumed present
        devs |= 0x02;            // mouse assumed present (enabled above)
        PS2_DEVICES.store(devs, Ordering::Release);
        LAST_ACTIVITY_TSC.store(crate::arch::rdtsc(), Ordering::Relaxed);
        PS2_READY.store(true, Ordering::Release);
    }

    log::info!(
        "[PS/2] controller ready — devices: kbd={} mouse={}",
        PS2_DEVICES.load(Ordering::Relaxed) & 0x01 != 0,
        PS2_DEVICES.load(Ordering::Relaxed) & 0x02 != 0,
    );
}

// ── Query helpers (used by syscall handlers) ──────────────────────────────────

/// Return the number of detected PS/2 input devices (0, 1, or 2).
pub fn device_count() -> u32 {
    let devs = PS2_DEVICES.load(Ordering::Relaxed);
    (devs & 0x01 != 0) as u32 + (devs & 0x02 != 0) as u32
}

/// Packed device info for device `n` (0-based).
///
/// Bit layout:
///   bits [3:0]   = type (1=keyboard, 2=mouse)
///   bits [11:4]  = IRQ number
///   bits [15:12] = interface (0=PS/2, 1=USB)
///
/// Returns 0 if `n` is out of range.
pub fn device_info_packed(n: u32) -> u32 {
    let devs = PS2_DEVICES.load(Ordering::Relaxed);
    let has_kbd   = devs & 0x01 != 0;
    let has_mouse = devs & 0x02 != 0;

    // Enumerate in order: keyboard first, then mouse.
    let mut idx = 0u32;
    if has_kbd {
        if idx == n {
            // type=1, IRQ=1, interface=PS/2 (0)
            return (1u32) | (1 << 4);
        }
        idx += 1;
    }
    if has_mouse && idx == n {
        // type=2, IRQ=12, interface=PS/2 (0)
        return (2u32) | (12 << 4);
    }
    0
}
