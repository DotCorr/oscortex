//! xHCI runtime — command/event rings, HID enumeration (keyboard + mouse boot
//! protocol), control transfers, and the HID interrupt-IN report path.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::mmio;
use crate::drivers::common::usb_hid;

use super::usb::XhciController;

const TRB_CYCLE: u32 = 1;
const TRB_IOC: u32 = 1 << 5;
const TRB_DIR_IN: u32 = 1 << 16;

const TRB_TYPE_NORMAL: u32 = 1 << 10;
const TRB_TYPE_SETUP: u32 = 2 << 10;
const TRB_TYPE_DATA: u32 = 3 << 10;
const TRB_TYPE_STATUS: u32 = 4 << 10;
const TRB_TYPE_LINK: u32 = 6 << 10;
const TRB_IDT: u32 = 1 << 6; // Immediate Data (Setup stage carries the 8 setup bytes)
const TRB_TC: u32 = 1 << 1; // Toggle Cycle (on Link TRB)
const TRB_TYPE_CMD_ENABLE_SLOT: u32 = 9 << 10;
const TRB_TYPE_CMD_ADDR_DEV: u32 = 11 << 10;
const TRB_TYPE_CMD_CONF_EP: u32 = 12 << 10;
const TRB_TYPE_EVT_TRANSFER: u32 = 32 << 10;
const TRB_TYPE_EVT_CMD_COMP: u32 = 33 << 10;

const CMD_TRBS: usize = 64;
const EVT_TRBS: usize = 256;
const EP_TRBS: usize = 32;
const HID_LEN: usize = 8;

const USBCMD_RUN: u32 = 1 << 0;
const USBCMD_HCRST: u32 = 1 << 1;
const USBSTS_HCH: u32 = 1 << 12;
const PORTSC_CCS: u32 = 1 << 0;
const PORTSC_PED: u32 = 1 << 1;
const PORTSC_PR: u32 = 1 << 4;

fn max_scratchpads(hcsparams2: u32) -> u32 {
    ((hcsparams2 >> 16) & 0x3e0) | ((hcsparams2 >> 27) & 0x1f)
}

fn eff_ports(max_ports: u8) -> u8 {
    if max_ports == 0 {
        8
    } else {
        max_ports
    }
}

unsafe fn setup_scratchpads(ctrl: &XhciController, cap_len: u8, dcbaa: *mut DcbaaPage) -> Result<(), &'static str> {
    let hcsparams2 = mmio::read32(ctrl.bar_virt, cap_len as usize + 4);
    let num_sp = max_scratchpads(hcsparams2);
    if num_sp == 0 {
        return Ok(());
    }
    let sp_array_phys = crate::mm::frame_allocator::alloc_frame().ok_or("sp-array")?;
    zpage(sp_array_phys);
    let sp_array = p2v(sp_array_phys) as *mut u64;
    for i in 0..num_sp {
        let buf = crate::mm::frame_allocator::alloc_frame().ok_or("sp-buf")?;
        zpage(buf);
        core::ptr::write(sp_array.add(i as usize), buf);
    }
    (*dcbaa).ptrs[0] = sp_array_phys;
    log::info!("[USB] XHCI scratchpad buffers={}", num_sp);
    Ok(())
}

#[repr(C, align(4096))]
struct TrbPage {
    trbs: [Trb; CMD_TRBS],
}

#[repr(C, align(4096))]
struct EventPage {
    trbs: [Trb; EVT_TRBS],
}

#[repr(C, align(4096))]
struct EpRingPage {
    trbs: [Trb; EP_TRBS],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Trb {
    dw0: u32,
    dw1: u32,
    dw2: u32,
    dw3: u32,
}

#[repr(C, align(4096))]
struct InputCtx {
    raw: [u8; 512],
}

#[repr(C, align(4096))]
struct DcbaaPage {
    ptrs: [u64; 256],
}

#[repr(C)]
struct ErstEntry {
    base: u64,
    size: u32,
    _rsvd: u32,
}

pub struct XhciRuntime {
    bar_virt: u64,
    cap_len: u8,
    rts_off: u32,
    db_off: u32,
    max_ports: u8,
    cmd_phys: u64,
    evt_phys: u64,
    erst_phys: u64,
    dcbaa_phys: u64,
    in_phys: u64,
    dev_phys: u64,
    ep_ring_phys: u64,
    ep0_ring_phys: u64,
    hid_phys: u64,
    ctrl_buf_phys: u64,
    cmd: *mut Trb,
    evt: *mut Trb,
    ep_ring: *mut Trb,
    ep0_ring: *mut Trb,
    cmd_idx: u32,
    cmd_cycle: u32,
    evt_idx: u32,
    evt_cycle: u32,
    ep_idx: u32,
    ep_cycle: u32,
    ep0_idx: u32,
    ep0_cycle: u32,
    slot: u8,
    hid_protocol: u8,
    cmd_done: bool,
    ctrl_done: bool,
    hid_armed: bool,
    enumerated: bool,
    enum_gave_up: bool,
    last_hid: [u8; HID_LEN],
    cur_x: i32,
    cur_y: i32,
}

static RUNTIME_OK: AtomicBool = AtomicBool::new(false);
static LIVE_KEY: AtomicBool = AtomicBool::new(false);
static mut RUNTIME: Option<XhciRuntime> = None;

fn hhdm() -> u64 {
    crate::mm::frame_allocator::hhdm_offset()
}

fn p2v(phys: u64) -> u64 {
    phys + hhdm()
}

unsafe fn zpage(phys: u64) {
    core::ptr::write_bytes(p2v(phys) as *mut u8, 0, 4096);
}

unsafe fn put_trb(ring: *mut Trb, idx: u32, cap: usize, trb: Trb, cycle: u32) {
    let i = (idx as usize) % cap;
    let t = &mut *ring.add(i);
    t.dw0 = trb.dw0;
    t.dw1 = trb.dw1;
    t.dw2 = trb.dw2;
    t.dw3 = (trb.dw3 & !1) | cycle;
}

pub fn live_key_seen() -> bool {
    LIVE_KEY.load(Ordering::Acquire)
}

pub fn runtime_ready() -> bool {
    RUNTIME_OK.load(Ordering::Acquire)
}

pub fn start(ctrl: &XhciController) -> Result<(), &'static str> {
    unsafe {
        let cap = mmio::read32(ctrl.bar_virt, 0);
        let cap_len = (cap & 0xFF) as u8;
        // DBOFF (0x14) and RTSOFF (0x18) are CAPABILITY registers at fixed offsets
        // from bar_virt — NOT relative to CAPLENGTH (that's the operational-reg
        // base). Reading them cap_len-relative pointed the doorbell + interrupter
        // at garbage, so commands never rang and no events were ever posted.
        let db_off = mmio::read32(ctrl.bar_virt, 0x14) & 0xFFFF_FFE0;
        let rts_off = mmio::read32(ctrl.bar_virt, 0x18) & 0xFFFF_FFE0;

        let cmd_phys = crate::mm::frame_allocator::alloc_frame().ok_or("cmd")?;
        let evt_phys = crate::mm::frame_allocator::alloc_frame().ok_or("evt")?;
        let erst_phys = crate::mm::frame_allocator::alloc_frame().ok_or("erst")?;
        let dcbaa_phys = crate::mm::frame_allocator::alloc_frame().ok_or("dcbaa")?;
        let in_phys = crate::mm::frame_allocator::alloc_frame().ok_or("in")?;
        let dev_phys = crate::mm::frame_allocator::alloc_frame().ok_or("dev")?;
        let ep_ring_phys = crate::mm::frame_allocator::alloc_frame().ok_or("ep")?;
        let ep0_ring_phys = crate::mm::frame_allocator::alloc_frame().ok_or("ep0")?;
        let hid_phys = crate::mm::frame_allocator::alloc_frame().ok_or("hid")?;
        let ctrl_buf_phys = crate::mm::frame_allocator::alloc_frame().ok_or("cbuf")?;

        for p in [
            cmd_phys,
            evt_phys,
            erst_phys,
            dcbaa_phys,
            in_phys,
            dev_phys,
            ep_ring_phys,
            ep0_ring_phys,
            hid_phys,
            ctrl_buf_phys,
        ] {
            zpage(p);
        }

        let cmd = (p2v(cmd_phys) as *mut TrbPage).as_mut().unwrap().trbs.as_mut_ptr();
        let evt = (p2v(evt_phys) as *mut EventPage)
            .as_mut()
            .unwrap()
            .trbs
            .as_mut_ptr();
        let ep_ring = (p2v(ep_ring_phys) as *mut EpRingPage)
            .as_mut()
            .unwrap()
            .trbs
            .as_mut_ptr();
        let ep0_ring = (p2v(ep0_ring_phys) as *mut EpRingPage)
            .as_mut()
            .unwrap()
            .trbs
            .as_mut_ptr();
        // Link TRB at the end of each transfer ring → wrap to start + toggle cycle,
        // so the rings keep running past EP_TRBS entries (the interrupt EP re-arms
        // forever; without this the controller halts after the first wrap).
        put_trb(ep_ring, (EP_TRBS - 1) as u32, EP_TRBS,
            Trb { dw0: ep_ring_phys as u32, dw1: (ep_ring_phys >> 32) as u32, dw2: 0,
                  dw3: TRB_TYPE_LINK | TRB_TC }, TRB_CYCLE);
        put_trb(ep0_ring, (EP_TRBS - 1) as u32, EP_TRBS,
            Trb { dw0: ep0_ring_phys as u32, dw1: (ep0_ring_phys >> 32) as u32, dw2: 0,
                  dw3: TRB_TYPE_LINK | TRB_TC }, TRB_CYCLE);

        let dcbaa = p2v(dcbaa_phys) as *mut DcbaaPage;
        core::ptr::write_bytes(dcbaa as *mut u8, 0, 4096);
        setup_scratchpads(ctrl, cap_len, dcbaa)?;

        let erst = p2v(erst_phys) as *mut ErstEntry;
        (*erst).base = evt_phys;
        (*erst).size = EVT_TRBS as u32;

        let op = ctrl.bar_virt + cap_len as u64;
        let ir0 = ctrl.bar_virt + rts_off as u64 + 0x20;

        // Halt and reset before reprogramming rings.
        let mut usbcmd = mmio::read32(op, 0x00);
        if usbcmd & USBCMD_RUN != 0 {
            mmio::write32(op, 0x00, usbcmd & !USBCMD_RUN);
            for _ in 0..500_000 {
                if mmio::read32(op, 0x04) & USBSTS_HCH != 0 {
                    break;
                }
                crate::arch::spin_pause();
            }
        }
        usbcmd = mmio::read32(op, 0x00);
        mmio::write32(op, 0x00, usbcmd | USBCMD_HCRST);
        for _ in 0..500_000 {
            usbcmd = mmio::read32(op, 0x00);
            if usbcmd & USBCMD_HCRST == 0 {
                break;
            }
            crate::arch::spin_pause();
        }

        mmio::write32(op, 0x38, ctrl.max_slots.max(1) as u32);
        mmio::write64(op, 0x30, dcbaa_phys);
        mmio::write64(op, 0x18, cmd_phys | 1);
        mmio::write32(ir0, 0x00, 1 << 1);
        mmio::write32(ir0, 0x08, 1);
        mmio::write64(ir0, 0x10, erst_phys);
        mmio::write64(ir0, 0x18, evt_phys | 1);

        let usbcmd = mmio::read32(op, 0x00);
        mmio::write32(op, 0x00, usbcmd | USBCMD_RUN);

        for _ in 0..500_000 {
            if mmio::read32(op, 0x04) & USBSTS_HCH == 0 {
                break;
            }
            crate::arch::spin_pause();
        }

        RUNTIME = Some(XhciRuntime {
            bar_virt: ctrl.bar_virt,
            cap_len,
            rts_off,
            db_off,
            max_ports: eff_ports(ctrl.max_ports),
            cmd_phys,
            evt_phys,
            erst_phys,
            dcbaa_phys,
            in_phys,
            dev_phys,
            ep_ring_phys,
            ep0_ring_phys,
            hid_phys,
            ctrl_buf_phys,
            cmd,
            evt,
            ep_ring,
            ep0_ring,
            cmd_idx: 0,
            cmd_cycle: TRB_CYCLE,
            evt_idx: 0,
            evt_cycle: TRB_CYCLE,
            ep_idx: 0,
            ep_cycle: TRB_CYCLE,
            ep0_idx: 0,
            ep0_cycle: TRB_CYCLE,
            slot: 0,
            hid_protocol: 0,
            cmd_done: false,
            ctrl_done: false,
            hid_armed: false,
            enumerated: false,
            enum_gave_up: false,
            last_hid: [0; HID_LEN],
            cur_x: 64,
            cur_y: 64,
        });
        RUNTIME_OK.store(true, Ordering::Release);
        log::info!("[USB] XHCI runtime started");

        let rt = RUNTIME.as_mut().unwrap();
        for _ in 0..100 {
            drain_events(rt);
            if try_enumerate(rt) {
                rt.enumerated = true;
                log::info!("[USB-HID] device enumerated slot={} protocol={}", rt.slot, rt.hid_protocol);
                arm_hid_transfer(rt);
                break;
            }
            if rt.enum_gave_up {
                break;
            }
            crate::arch::spin_pause();
        }
        if !RUNTIME.as_ref().unwrap().enumerated {
            log::warn!("[USB-HID] device enumeration pending (poll on vsync)");
        }

        Ok(())
    }
}

pub fn poll() {
    if !RUNTIME_OK.load(Ordering::Acquire) {
        return;
    }
    unsafe {
        let rt = match RUNTIME.as_mut() {
            Some(r) => r,
            None => return,
        };
        rt.enum_gave_up = false;
        drain_events(rt);
        if !rt.enumerated {
            if try_enumerate(rt) {
                rt.enumerated = true;
                log::info!("[USB-HID] device enumerated slot={} protocol={}", rt.slot, rt.hid_protocol);
            }
        } else if !rt.hid_armed {
            arm_hid_transfer(rt);
        }
    }
}

unsafe fn op(rt: &XhciRuntime) -> u64 {
    rt.bar_virt + rt.cap_len as u64
}

unsafe fn portsc(rt: &XhciRuntime, port: u8) -> u32 {
    mmio::read32(op(rt), 0x400 + ((port as usize) - 1) * 0x10)
}

unsafe fn set_portsc(rt: &XhciRuntime, port: u8, val: u32) {
    mmio::write32(op(rt), 0x400 + ((port as usize) - 1) * 0x10, val);
}

unsafe fn post_cmd(rt: &mut XhciRuntime, trb: Trb) {
    put_trb(rt.cmd, rt.cmd_idx, CMD_TRBS, trb, rt.cmd_cycle);
    rt.cmd_idx += 1;
    if rt.cmd_idx as usize >= CMD_TRBS {
        rt.cmd_idx = 0;
        rt.cmd_cycle ^= TRB_CYCLE;
    }
    // CRCR (op 0x18) is programmed ONCE at init; the controller advances its own
    // command-ring dequeue pointer. Rewriting it here clobbered the ring and the
    // command was never consumed. Just publish the TRB and ring the doorbell.
    crate::arch::memory_fence();
    let db = rt.bar_virt + rt.db_off as u64;
    mmio::write32(db, 0, 0);
}

unsafe fn wait_cmd(rt: &mut XhciRuntime) -> bool {
    rt.cmd_done = false;
    for _ in 0..2_000_000 {
        drain_events(rt);
        if rt.cmd_done {
            rt.cmd_done = false;
            return true;
        }
        crate::arch::spin_pause();
    }
    false
}

/// Post the three (or two) control-transfer stages on the EP0 ring and ring the
/// slot's EP0 doorbell (DCI 1), then wait for the Status-stage transfer event.
unsafe fn control_xfer(
    rt: &mut XhciRuntime,
    bm: u8,
    req: u8,
    value: u16,
    index: u16,
    length: u16,
    data_phys: u64,
) -> bool {
    let setup_lo = (bm as u32) | ((req as u32) << 8) | ((value as u32) << 16);
    let setup_hi = (index as u32) | ((length as u32) << 16);
    let dir_in = bm & 0x80 != 0;
    let trt = if length == 0 { 0 } else if dir_in { 3u32 } else { 2u32 };

    put_ep0(rt, Trb {
        dw0: setup_lo,
        dw1: setup_hi,
        dw2: 8,
        dw3: TRB_TYPE_SETUP | TRB_IDT | (trt << 16),
    });
    if length > 0 {
        let dir = if dir_in { TRB_DIR_IN } else { 0 };
        put_ep0(rt, Trb {
            dw0: data_phys as u32,
            dw1: (data_phys >> 32) as u32,
            dw2: length as u32,
            dw3: TRB_TYPE_DATA | dir,
        });
    }
    // Status stage: direction opposite the data stage; IN for no-data / OUT data.
    let status_dir = if length > 0 && dir_in { 0 } else { TRB_DIR_IN };
    put_ep0(rt, Trb {
        dw0: 0,
        dw1: 0,
        dw2: 0,
        dw3: TRB_TYPE_STATUS | status_dir | TRB_IOC,
    });

    crate::arch::memory_fence();
    let db = rt.bar_virt + rt.db_off as u64 + (rt.slot as u64) * 4;
    mmio::write32(db, 0, 1); // EP0 = DCI 1

    rt.ctrl_done = false;
    for _ in 0..2_000_000 {
        drain_events(rt);
        if rt.ctrl_done {
            rt.ctrl_done = false;
            return true;
        }
        crate::arch::spin_pause();
    }
    false
}

/// Append a TRB to the EP0 control ring, wrapping at the trailing Link TRB.
unsafe fn put_ep0(rt: &mut XhciRuntime, trb: Trb) {
    put_trb(rt.ep0_ring, rt.ep0_idx, EP_TRBS, trb, rt.ep0_cycle);
    rt.ep0_idx += 1;
    if rt.ep0_idx as usize >= EP_TRBS - 1 {
        rt.ep0_idx = 0;
        rt.ep0_cycle ^= TRB_CYCLE;
    }
}

unsafe fn drain_events(rt: &mut XhciRuntime) -> bool {
    let mut cmd_done = false;
    loop {
        let i = (rt.evt_idx as usize) % EVT_TRBS;
        let dw3 = (*rt.evt.add(i)).dw3;
        if (dw3 & 1) != rt.evt_cycle {
            break;
        }
        // TRB Type is a 6-bit field (bits 15:10). A 0x3FF mask would spill into
        // the Endpoint ID field (bits 20:16) of Transfer Events, so EP0 (id 1) and
        // HID (id 3) events would be misclassified and dropped — only command/port
        // events (endpoint-id bits = 0) would match. Mask exactly 6 bits.
        let kind = dw3 & (0x3F << 10);
        if kind == TRB_TYPE_EVT_CMD_COMP {
            cmd_done = true;
            rt.cmd_done = true;
            let slot = (dw3 >> 24) & 0xFF;
            if rt.slot == 0 && slot != 0 {
                rt.slot = slot as u8;
                let dcbaa = p2v(rt.dcbaa_phys) as *mut DcbaaPage;
                (*dcbaa).ptrs[rt.slot as usize] = rt.dev_phys;
            }
        } else if kind == TRB_TYPE_EVT_TRANSFER {
            // Endpoint ID 1 = EP0 (control); higher = HID interrupt IN (EP1=DCI 3).
            let ep_id = (dw3 >> 16) & 0x1F;
            if ep_id <= 1 {
                rt.ctrl_done = true;
            } else {
                let report = core::slice::from_raw_parts(p2v(rt.hid_phys) as *const u8, HID_LEN);
                route_report(report, rt);
                rt.hid_armed = false;
            }
        }
        rt.evt_idx += 1;
        if rt.evt_idx as usize >= EVT_TRBS {
            rt.evt_idx = 0;
            rt.evt_cycle ^= TRB_CYCLE;
        }
        let ir0 = rt.bar_virt + rt.rts_off as u64 + 0x20;
        let erdp = rt.evt_phys + (rt.evt_idx as u64) * 16;
        mmio::write64(ir0, 0x18, erdp | rt.evt_cycle as u64);
    }
    cmd_done
}

fn route_report(report: &[u8], rt: &mut XhciRuntime) {
    // Protocol 2 = boot mouse. Mouse deltas are RELATIVE — two identical reports
    // are two real movements, so never dedup them (unlike keyboards, where an
    // unchanged report means no key state change).
    if rt.hid_protocol == 2 {
        route_mouse(report, rt);
        return;
    }
    if report == &rt.last_hid {
        return;
    }
    let old = rt.last_hid;
    rt.last_hid.copy_from_slice(report);

    if rt.hid_protocol == 1 || rt.hid_protocol == 0 {
        if let Some((sc, pressed)) = usb_hid::handle_boot_keyboard_report(report) {
            crate::wm::push_key(sc, pressed);
            LIVE_KEY.store(true, Ordering::Release);
            log::info!("[USB-HID] live key scancode={:#x}", sc);
        }
        for i in 2..8 {
            let prev = old.get(i).copied().unwrap_or(0);
            let cur = report.get(i).copied().unwrap_or(0);
            if prev != 0 && prev != cur {
                if let Some(sc) = usb_hid::usb_keycode_to_scancode(prev) {
                    crate::wm::push_key(sc, false);
                }
            }
        }
    }
}

/// Boot-protocol mouse report: byte0 = buttons (b0 L, b1 R, b2 M),
/// byte1 = dx (i8), byte2 = dy (i8), byte3 = wheel (i8). Maintain an absolute
/// cursor clamped to the framebuffer and feed the WM's unified pointer state.
fn route_mouse(report: &[u8], rt: &mut XhciRuntime) {
    let buttons = (report[0] as u32) & 0x7;
    let dx = report[1] as i8 as i32;
    let dy = report[2] as i8 as i32;
    let wheel = report.get(3).map(|b| *b as i8 as i32).unwrap_or(0);
    let (w, h) = crate::drivers::fb::size_px().unwrap_or((1024, 768));
    rt.cur_x = (rt.cur_x + dx).clamp(0, w as i32 - 1);
    rt.cur_y = (rt.cur_y + dy).clamp(0, h as i32 - 1);
    crate::wm::push_pointer(rt.cur_x, rt.cur_y, buttons);
    if wheel != 0 {
        crate::wm::push_scroll(rt.cur_x, rt.cur_y, wheel);
    }
}

unsafe fn write_input_address_ctx(rt: &XhciRuntime, port: u8, speed: u32) {
    let p = p2v(rt.in_phys) as *mut u8;
    core::ptr::write_bytes(p, 0, 512);
    // Input Control Context @ 0x00: add Slot (bit0) + EP0 (bit1).
    core::ptr::write_unaligned(p.add(0x04) as *mut u32, 0x03);
    // Slot Context @ 0x20: Context Entries=1 (bits31:27), Speed (bits23:20).
    core::ptr::write_unaligned(p.add(0x20) as *mut u32, (1u32 << 27) | (speed << 20));
    // Root Hub Port Number @ dword1 bits31:16.
    core::ptr::write_unaligned(p.add(0x24) as *mut u32, (port as u32) << 16);
    // EP0 Max Packet Size depends on bus speed (xHCI speed IDs: 1=Full, 2=Low,
    // 3=High, 4=Super). High-speed control endpoints REQUIRE 64; a mismatch halts
    // EP0 after the first transfer. Low=8, Super=512, Full/High=64.
    let mps0: u32 = match speed {
        2 => 8,
        4 => 512,
        _ => 64,
    };
    // EP0 Context @ 0x40, dword1: CErr=3 (bits2:1), type 4=control (bits5:3),
    // Max Packet Size (bits31:16).
    core::ptr::write_unaligned(p.add(0x44) as *mut u32, (3u32 << 1) | (4u32 << 3) | (mps0 << 16));
    // EP0 TR Dequeue Pointer @ 0x48, DCS=1.
    core::ptr::write_unaligned(p.add(0x48) as *mut u64, rt.ep0_ring_phys | 1);
}

unsafe fn write_input_config_ep1(rt: &XhciRuntime) {
    let p = p2v(rt.in_phys) as *mut u8;
    core::ptr::write_bytes(p, 0, 512);
    // Add Slot (bit0, required by Configure Endpoint) + EP1-IN (DCI 3, bit3).
    core::ptr::write_unaligned(p.add(0x04) as *mut u32, (1u32 << 0) | (1u32 << 3));
    // Slot Context @ 0x20: Context Entries must reach the highest DCI = 3.
    core::ptr::write_unaligned(p.add(0x20) as *mut u32, 3u32 << 27);
    // EP1-IN Context @ DCI3 = 0x20 + 3*0x20 = 0x80.
    // dword0 @ 0x80: Interval (bits23:16).
    core::ptr::write_unaligned(p.add(0x80) as *mut u32, 8u32 << 16);
    // dword1 @ 0x84: CErr=3 (bits2:1), type 7=interrupt-IN (bits5:3),
    // Max Packet Size=8 (bits31:16).
    core::ptr::write_unaligned(p.add(0x84) as *mut u32, (3u32 << 1) | (7u32 << 3) | (8u32 << 16));
    // EP1 TR Dequeue Pointer @ 0x88, DCS=1.
    core::ptr::write_unaligned(p.add(0x88) as *mut u64, rt.ep_ring_phys | 1);
}

unsafe fn try_enumerate(rt: &mut XhciRuntime) -> bool {
    if rt.enumerated || rt.enum_gave_up {
        return rt.enumerated;
    }
    let mut port = 0u8;
    for p in 1..=rt.max_ports {
        let sc = portsc(rt, p);
        if sc & PORTSC_CCS != 0 {
            port = p;
            break;
        }
    }
    if port == 0 {
        return false;
    }

    let mut sc = portsc(rt, port);
    if sc & PORTSC_PED == 0 {
        set_portsc(rt, port, sc | PORTSC_PR);
        for _ in 0..200_000 {
            sc = portsc(rt, port);
            if sc & PORTSC_PR == 0 {
                break;
            }
            crate::arch::spin_pause();
        }
        sc = portsc(rt, port);
    }

    post_cmd(
        rt,
        Trb {
            dw0: 0,
            dw1: 0,
            dw2: 0,
            dw3: TRB_TYPE_CMD_ENABLE_SLOT | TRB_IOC | rt.cmd_cycle,
        },
    );
    if !wait_cmd(rt) || rt.slot == 0 {
        rt.enum_gave_up = true;
        return false;
    }
    // Port speed lives in PORTSC bits[13:10]; the slot context wants it verbatim.
    let speed = (portsc(rt, port) >> 10) & 0xF;

    // Address Device (BSR=0 → controller issues SET_ADDRESS on EP0).
    write_input_address_ctx(rt, port, speed);
    post_cmd(
        rt,
        Trb {
            dw0: rt.in_phys as u32,
            dw1: (rt.in_phys >> 32) as u32,
            dw2: 0,
            dw3: TRB_TYPE_CMD_ADDR_DEV | TRB_IOC | ((rt.slot as u32) << 24) | rt.cmd_cycle,
        },
    );
    if !wait_cmd(rt) {
        log::warn!("[USB-HID] address device failed");
        return false;
    }

    // GET_DESCRIPTOR(Configuration) → read bInterfaceProtocol (1=kbd, 2=mouse)
    // and bConfigurationValue so SET_CONFIGURATION uses the right value.
    let mut cfg_val = 1u8;
    if control_xfer(rt, 0x80, 0x06, 0x0200, 0, 64, rt.ctrl_buf_phys) {
        let buf = core::slice::from_raw_parts(p2v(rt.ctrl_buf_phys) as *const u8, 64);
        if buf[1] == 0x02 {
            cfg_val = buf[5];
        }
        // Walk descriptors for the (first) HID interface's bInterfaceProtocol.
        let total = buf[0] as usize;
        let mut off = total; // skip the 9-byte config descriptor
        while off + 2 <= 64 {
            let dlen = buf[off] as usize;
            let dtype = buf[off + 1];
            if dlen == 0 {
                break;
            }
            if dtype == 0x04 && off + 8 <= 64 {
                // Interface descriptor: bInterfaceProtocol @ +7.
                rt.hid_protocol = buf[off + 7];
                break;
            }
            off += dlen;
        }
    }
    if cfg_val == 0 {
        cfg_val = 1;
    }

    // SET_CONFIGURATION → move the device to the Configured state so its endpoints
    // become active (without this it never sends interrupt reports).
    if !control_xfer(rt, 0x00, 0x09, cfg_val as u16, 0, 0, 0) {
        log::warn!("[USB-HID] set configuration failed");
        return false;
    }

    // xHCI Configure Endpoint for the interrupt-IN endpoint (DCI 3).
    write_input_config_ep1(rt);
    post_cmd(
        rt,
        Trb {
            dw0: rt.in_phys as u32,
            dw1: (rt.in_phys >> 32) as u32,
            dw2: 0,
            dw3: TRB_TYPE_CMD_CONF_EP | TRB_IOC | ((rt.slot as u32) << 24) | rt.cmd_cycle,
        },
    );
    if !wait_cmd(rt) {
        log::warn!("[USB-HID] configure EP failed");
        return false;
    }

    // HID class requests: SET_PROTOCOL(boot=0) so reports are boot-format, and
    // SET_IDLE(0) so the device only reports on change. Best-effort.
    let _ = control_xfer(rt, 0x21, 0x0B, 0, 0, 0, 0); // SET_PROTOCOL boot
    let _ = control_xfer(rt, 0x21, 0x0A, 0, 0, 0, 0); // SET_IDLE infinite
    true
}

unsafe fn arm_hid_transfer(rt: &mut XhciRuntime) {
    put_trb(
        rt.ep_ring,
        rt.ep_idx,
        EP_TRBS,
        Trb {
            dw0: rt.hid_phys as u32,
            dw1: (rt.hid_phys >> 32) as u32,
            dw2: HID_LEN as u32,
            dw3: TRB_TYPE_NORMAL | TRB_IOC | TRB_DIR_IN | rt.ep_cycle,
        },
        rt.ep_cycle,
    );
    rt.ep_idx += 1;
    if rt.ep_idx as usize >= EP_TRBS - 1 {
        // Wrap before the trailing Link TRB (it toggles the cycle for us).
        rt.ep_idx = 0;
        rt.ep_cycle ^= TRB_CYCLE;
    }
    rt.hid_armed = true;
    crate::arch::memory_fence();
    // EP1 IN = DCI 3.
    let db = rt.bar_virt + rt.db_off as u64 + (rt.slot as u64) * 4;
    mmio::write32(db, 0, 3);
}
