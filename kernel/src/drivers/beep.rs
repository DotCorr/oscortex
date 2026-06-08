//! PC-speaker beep driver.
//!
//! Backs the Flutter `flutter/platform` `SystemSound.play` method with a real
//! audible tone on hardware that has a PC speaker. Uses PIT channel 2 (ports
//! 0x42/0x43) as the tone generator and gates it through the keyboard-controller
//! port 0x61 (bits 0–1 = speaker enable + gate).
//!
//! The tone is emitted synchronously: enable, busy-wait the (capped) duration,
//! disable. Durations are clamped so a misbehaving app cannot hold the speaker
//! on or stall the cooperative scheduler for long.

use crate::arch::port_io::{inb, outb};

const PIT_CH2_DATA: u16 = 0x42;
const PIT_CMD: u16 = 0x43;
const SPEAKER_PORT: u16 = 0x61;

/// PIT input clock (1.193182 MHz).
const PIT_FREQ: u32 = 1_193_182;

/// Hard cap on tone length so a beep can never stall the system.
const MAX_DURATION_MS: u64 = 250;

/// Play a tone of `freq_hz` for `duration_ms` (both clamped to safe bounds),
/// then silence the speaker. A `freq_hz` of 0 is treated as a default 800 Hz
/// system beep (matching `SystemSound.play` having no explicit frequency).
pub fn play(freq_hz: u32, duration_ms: u64) {
    let freq = if freq_hz == 0 { 800 } else { freq_hz.clamp(20, 20_000) };
    let dur = duration_ms.clamp(1, MAX_DURATION_MS);

    let divisor = (PIT_FREQ / freq) as u16;

    unsafe {
        // Program PIT channel 2 in mode 3 (square wave): command byte 0xB6.
        outb(PIT_CMD, 0xB6);
        outb(PIT_CH2_DATA, (divisor & 0xFF) as u8);
        outb(PIT_CH2_DATA, (divisor >> 8) as u8);

        // Enable the speaker: set bits 0 (gate) and 1 (data) of port 0x61.
        let cur = inb(SPEAKER_PORT);
        if (cur & 0b11) != 0b11 {
            outb(SPEAKER_PORT, cur | 0b11);
        }
    }

    // Busy-wait the (short, capped) duration using the TSC.
    let tsc_hz = crate::arch::apic::tsc_hz();
    let cycles = tsc_hz.saturating_mul(dur) / 1000;
    let start = crate::arch::rdtsc();
    while crate::arch::rdtsc().wrapping_sub(start) < cycles {
        core::hint::spin_loop();
    }

    unsafe {
        // Silence: clear bits 0–1 of port 0x61.
        let cur = inb(SPEAKER_PORT);
        outb(SPEAKER_PORT, cur & !0b11);
    }
}
