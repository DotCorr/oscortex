//! virtio-input (virtio-mmio, modern / v2) — aarch64 pointer input.
//!
//! On QEMU `-M virt` there is no PS/2 8042 controller, so the x86 input path
//! (drivers::ps2) does not apply. Input devices are attached to the virtio-mmio
//! transport instead. This driver brings up a `virtio-tablet-device` (absolute
//! pointer) and feeds hover/click into the shared WM event queue via
//! `wm::push_pointer`, exactly like the x86 PS/2 mouse path.
//!
//! QEMU virt virtio-mmio layout: 32 transport slots at 0x0a000000, stride 0x200.
//! Slot `i` raises GIC SPI (16 + i) → INTID (48 + i). The region is already
//! identity-mapped Device memory by the boot MMU (mmu.rs entry 0: 0x0..0x4000_0000).
//!
//! Scope: absolute pointer (move + buttons) only. Keyboard (evdev keycode →
//! scancode) is a follow-up.

#![allow(dead_code)]

use crate::drivers::common::vring::{VringLayout, VIRTQ_DESC_F_WRITE};
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

// ── virtio-mmio register offsets (modern, version 2) ───────────────────────────
const MAGIC: u64 = 0x000; // 'virt' = 0x74726976
const VERSION: u64 = 0x004; // 2 = modern
const DEVICE_ID: u64 = 0x008; // 18 = input
const DEVICE_FEATURES: u64 = 0x010;
const DEVICE_FEATURES_SEL: u64 = 0x014;
const DRIVER_FEATURES: u64 = 0x020;
const DRIVER_FEATURES_SEL: u64 = 0x024;
const GUEST_PAGE_SIZE: u64 = 0x028; // legacy: driver page size (4096)
const QUEUE_SEL: u64 = 0x030;
const QUEUE_NUM_MAX: u64 = 0x034;
const QUEUE_NUM: u64 = 0x038;
const QUEUE_ALIGN: u64 = 0x03c; // legacy: used-ring alignment (4096)
const QUEUE_PFN: u64 = 0x040; // legacy: ring page-frame number (phys >> 12)
const QUEUE_NOTIFY: u64 = 0x050;
const INTERRUPT_STATUS: u64 = 0x060;
const INTERRUPT_ACK: u64 = 0x064;
const STATUS: u64 = 0x070;

// Status bits.
const S_ACK: u32 = 1;
const S_DRIVER: u32 = 2;
const S_DRIVER_OK: u32 = 4;

const PAGE: u64 = 4096;

const MAGIC_VALUE: u32 = 0x7472_6976;
const DEVICE_ID_INPUT: u32 = 18;

const VIRT_MMIO_BASE: u64 = 0x0a00_0000;
const VIRT_MMIO_STRIDE: u64 = 0x200;
const VIRT_MMIO_SLOTS: u64 = 32;
const SPI_BASE_INTID: u32 = 48; // slot 0 → SPI 16 → INTID 48

const QSIZE: u16 = 8;
const EVENT_BYTES: u64 = 8; // virtio_input_event: u16 type, u16 code, u32 value

// evdev event types / codes.
const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const EV_ABS: u16 = 0x03;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const REL_X: u16 = 0x00;
const REL_Y: u16 = 0x01;
const REL_WHEEL: u16 = 0x08;
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;

// QEMU virtio-tablet reports ABS axes in [0, 0x7FFF].
const ABS_MAX: i64 = 0x7FFF;

#[inline]
unsafe fn r32(base: u64, off: u64) -> u32 {
    core::ptr::read_volatile((base + off) as *const u32)
}
#[inline]
unsafe fn w32(base: u64, off: u64, v: u32) {
    core::ptr::write_volatile((base + off) as *mut u32, v);
}

struct InputDev {
    base: u64,
    intid: u32,
    ring_phys: u64,
    layout: VringLayout,
    last_used: u16,
    buf_base: u64,   // physical/virtual (identity) base of the event buffers
    // accumulated pointer state, flushed on EV_SYN
    cur_x: i32,
    cur_y: i32,
    buttons: u32,
    scroll_dy: i32,
    is_abs: bool,
}

static DEVS: Mutex<heapless_vec::Vec> = Mutex::new(heapless_vec::Vec::new());

// Tiny fixed-capacity vec to avoid pulling alloc into the ISR path.
mod heapless_vec {
    use super::InputDev;
    pub struct Vec {
        items: [Option<InputDev>; 4],
        len: usize,
    }
    impl Vec {
        pub const fn new() -> Self {
            Self { items: [None, None, None, None], len: 0 }
        }
        pub fn push(&mut self, d: InputDev) {
            if self.len < self.items.len() {
                self.items[self.len] = Some(d);
                self.len += 1;
            }
        }
        pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut InputDev> {
            self.items.iter_mut().filter_map(|o| o.as_mut())
        }
    }
}

static DEV_COUNT: AtomicU32 = AtomicU32::new(0);

/// True if `intid` belongs to one of our virtio-input devices.
pub fn owns_intid(intid: u32) -> bool {
    (SPI_BASE_INTID..SPI_BASE_INTID + VIRT_MMIO_SLOTS as u32).contains(&intid)
        && DEV_COUNT.load(Ordering::Relaxed) > 0
}

/// Lay out and arm one device's event virtqueue, post receive buffers, and set
/// DRIVER_OK. Returns the ring's physical base + buffer base on success.
unsafe fn setup_queue(base: u64) -> Option<(u64, VringLayout, u64)> {
    w32(base, QUEUE_SEL, 0);
    let qmax = r32(base, QUEUE_NUM_MAX);
    if qmax == 0 {
        return None;
    }
    let qsize = QSIZE.min(qmax as u16);
    let layout = VringLayout::for_qsize(qsize)?;
    // Legacy virtio-mmio: the whole split-ring (desc + avail + page-aligned used)
    // lives in one contiguous, page-aligned region described by a single QueuePFN.
    // We append one extra page for the event receive buffers.
    let pages = layout.page_count + 1;
    let ring_phys = crate::mm::frame_allocator::alloc_contiguous_frames(pages)?;
    core::ptr::write_bytes(ring_phys as *mut u8, 0, pages * PAGE as usize);
    let buf_base = ring_phys + (layout.page_count as u64) * PAGE;

    // Descriptors: each points at an 8-byte event buffer, device-writable.
    for i in 0..qsize as u64 {
        let d = ring_phys + i * 16;
        core::ptr::write_volatile(d as *mut u64, buf_base + i * EVENT_BYTES); // addr
        core::ptr::write_volatile((d + 8) as *mut u32, EVENT_BYTES as u32); // len
        core::ptr::write_volatile((d + 12) as *mut u16, VIRTQ_DESC_F_WRITE); // flags
        core::ptr::write_volatile((d + 14) as *mut u16, 0); // next
        let slot = ring_phys + layout.avail_slot_offset(i as u16) as u64;
        core::ptr::write_volatile(slot as *mut u16, i as u16);
    }
    // avail.idx = qsize (all buffers offered to the device).
    let avail_idx = ring_phys + layout.avail_idx_off as u64;
    core::ptr::write_volatile(avail_idx as *mut u16, qsize);

    w32(base, GUEST_PAGE_SIZE, PAGE as u32);
    w32(base, QUEUE_NUM, qsize as u32);
    w32(base, QUEUE_ALIGN, PAGE as u32);
    w32(base, QUEUE_PFN, (ring_phys / PAGE) as u32);

    Some((ring_phys, layout, buf_base))
}

/// Probe all 32 virtio-mmio slots; bring up every virtio-input device found.
pub fn init() {
    let mut found = 0u32;
    for slot in 0..VIRT_MMIO_SLOTS {
        let base = VIRT_MMIO_BASE + slot * VIRT_MMIO_STRIDE;
        unsafe {
            if r32(base, MAGIC) != MAGIC_VALUE {
                continue;
            }
            let ver = r32(base, VERSION);
            let dev_id = r32(base, DEVICE_ID);
            if dev_id == 0 {
                continue; // empty slot
            }
            log::error!("[virtio-input] probe slot={} base={:#x} magic=ok ver={} id={}", slot, base, ver, dev_id);
            if dev_id != DEVICE_ID_INPUT || ver != 1 {
                continue; // not a legacy (v1) virtio-input device
            }
            log::error!("[virtio-input] slot={} base={:#x} ver={} id={} → init", slot, base, ver, dev_id);

            // Legacy virtio-mmio bring-up: reset → ACK → DRIVER → (features=0) →
            // queue → DRIVER_OK. No FEATURES_OK / VERSION_1 negotiation (modern-only).
            w32(base, STATUS, 0);
            let mut status = S_ACK;
            w32(base, STATUS, status);
            status |= S_DRIVER;
            w32(base, STATUS, status);

            // Accept no optional features (the input device needs none for events).
            w32(base, DEVICE_FEATURES_SEL, 0);
            let _df0 = r32(base, DEVICE_FEATURES);
            w32(base, DRIVER_FEATURES_SEL, 0);
            w32(base, DRIVER_FEATURES, 0);

            let Some((ring_phys, layout, buf_base)) = setup_queue(base) else {
                log::error!("[virtio-input] slot={} queue setup failed", slot);
                continue;
            };

            status |= S_DRIVER_OK;
            w32(base, STATUS, status);

            // Enable this slot's SPI at the GIC (level-triggered, routed to CPU0).
            let intid = SPI_BASE_INTID + slot as u32;
            crate::arch::aarch64::gic::set_priority(intid, 0xA0);
            crate::arch::aarch64::gic::route_to_cpu0(intid);
            crate::arch::aarch64::gic::enable_irq(intid);
            // Kick the device once so it knows buffers are available.
            w32(base, QUEUE_NOTIFY, 0);

            DEVS.lock().push(InputDev {
                base,
                intid,
                ring_phys,
                layout,
                last_used: 0,
                buf_base,
                cur_x: 0,
                cur_y: 0,
                buttons: 0,
                scroll_dy: 0,
                is_abs: false,
            });
            found += 1;
            log::error!("[virtio-input] slot={} READY intid={} qsize={}", slot, intid, layout.qsize);
        }
    }
    DEV_COUNT.store(found, Ordering::Release);
    log::error!("[virtio-input] init complete: {} device(s)", found);
}

/// Drain the used ring of every device, translating evdev events into WM events.
/// Called from the aarch64 IRQ handler when a virtio-mmio SPI fires.
pub fn handle_irq() {
    let mut devs = DEVS.lock();
    let (w, h) = crate::drivers::fb::size_px().unwrap_or((1280, 800));
    for d in devs.iter_mut() {
        unsafe {
            // Ack the device interrupt.
            let isr = r32(d.base, INTERRUPT_STATUS);
            if isr != 0 {
                w32(d.base, INTERRUPT_ACK, isr);
            }
            let used_idx_addr = d.ring_phys + d.layout.used_idx_off as u64;
            loop {
                let cur_used = core::ptr::read_volatile(used_idx_addr as *const u16);
                if cur_used == d.last_used {
                    break;
                }
                let elem_off = d.layout.used_elem_offset(d.last_used);
                let elem = d.ring_phys + elem_off as u64;
                let desc_id = core::ptr::read_volatile(elem as *const u32) & (d.layout.qsize as u32 - 1);
                let buf = d.buf_base + desc_id as u64 * EVENT_BYTES;
                let etype = core::ptr::read_volatile(buf as *const u16);
                let code = core::ptr::read_volatile((buf + 2) as *const u16);
                let value = core::ptr::read_volatile((buf + 4) as *const u32) as i32;

                match etype {
                    EV_ABS => {
                        d.is_abs = true;
                        let scaled = |v: i32, span: i64| -> i32 {
                            ((v as i64 * (span - 1)) / ABS_MAX) as i32
                        };
                        match code {
                            ABS_X => d.cur_x = scaled(value, w as i64),
                            ABS_Y => d.cur_y = scaled(value, h as i64),
                            _ => {}
                        }
                    }
                    EV_REL => match code {
                        REL_X => d.cur_x = (d.cur_x + value).clamp(0, w as i32 - 1),
                        REL_Y => d.cur_y = (d.cur_y + value).clamp(0, h as i32 - 1),
                        REL_WHEEL => d.scroll_dy += value,
                        _ => {}
                    },
                    EV_KEY => {
                        let mask = match code {
                            BTN_LEFT => 1,
                            BTN_RIGHT => 2,
                            BTN_MIDDLE => 4,
                            _ => 0,
                        };
                        if mask != 0 {
                            if value != 0 {
                                d.buttons |= mask;
                            } else {
                                d.buttons &= !mask;
                            }
                        }
                    }
                    EV_SYN => {
                        crate::wm::push_pointer(d.cur_x, d.cur_y, d.buttons);
                        if d.scroll_dy != 0 {
                            crate::wm::push_scroll(d.cur_x, d.cur_y, d.scroll_dy);
                            d.scroll_dy = 0;
                        }
                    }
                    _ => {}
                }

                // Re-offer this buffer to the device.
                let avail_idx_addr = d.ring_phys + d.layout.avail_idx_off as u64;
                let ai = core::ptr::read_volatile(avail_idx_addr as *const u16);
                let slot = d.ring_phys + d.layout.avail_slot_offset(ai) as u64;
                core::ptr::write_volatile(slot as *mut u16, desc_id as u16);
                core::ptr::write_volatile(avail_idx_addr as *mut u16, ai.wrapping_add(1));
                d.last_used = d.last_used.wrapping_add(1);
            }
            // Notify the device that buffers were replenished.
            w32(d.base, QUEUE_NOTIFY, 0);
        }
    }
}
