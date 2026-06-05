//! xHCI runtime — command/event rings, keyboard enumeration, HID interrupt IN.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::mmio;
use crate::drivers::common::usb_hid;

use super::usb::XhciController;

const TRB_CYCLE: u32 = 1;
const TRB_IOC: u32 = 1 << 5;
const TRB_DIR_IN: u32 = 1 << 16;

const TRB_TYPE_NORMAL: u32 = 1 << 10;
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
    hid_phys: u64,
    cmd: *mut Trb,
    evt: *mut Trb,
    ep_ring: *mut Trb,
    cmd_idx: u32,
    cmd_cycle: u32,
    evt_idx: u32,
    evt_cycle: u32,
    ep_idx: u32,
    ep_cycle: u32,
    slot: u8,
    hid_armed: bool,
    enumerated: bool,
    enum_gave_up: bool,
    last_hid: [u8; HID_LEN],
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
        let rts_off = mmio::read32(ctrl.bar_virt, cap_len as usize + 0x18) & 0xFFFF_FFE0;
        let db_off = mmio::read32(ctrl.bar_virt, cap_len as usize + 0x14) & 0xFFFF_FFE0;

        let cmd_phys = crate::mm::frame_allocator::alloc_frame().ok_or("cmd")?;
        let evt_phys = crate::mm::frame_allocator::alloc_frame().ok_or("evt")?;
        let erst_phys = crate::mm::frame_allocator::alloc_frame().ok_or("erst")?;
        let dcbaa_phys = crate::mm::frame_allocator::alloc_frame().ok_or("dcbaa")?;
        let in_phys = crate::mm::frame_allocator::alloc_frame().ok_or("in")?;
        let dev_phys = crate::mm::frame_allocator::alloc_frame().ok_or("dev")?;
        let ep_ring_phys = crate::mm::frame_allocator::alloc_frame().ok_or("ep")?;
        let hid_phys = crate::mm::frame_allocator::alloc_frame().ok_or("hid")?;

        for p in [
            cmd_phys,
            evt_phys,
            erst_phys,
            dcbaa_phys,
            in_phys,
            dev_phys,
            ep_ring_phys,
            hid_phys,
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
            hid_phys,
            cmd,
            evt,
            ep_ring,
            cmd_idx: 0,
            cmd_cycle: TRB_CYCLE,
            evt_idx: 0,
            evt_cycle: TRB_CYCLE,
            ep_idx: 0,
            ep_cycle: TRB_CYCLE,
            slot: 0,
            hid_armed: false,
            enumerated: false,
            enum_gave_up: false,
            last_hid: [0; HID_LEN],
        });
        RUNTIME_OK.store(true, Ordering::Release);
        log::info!("[USB] XHCI runtime started");

        let rt = RUNTIME.as_mut().unwrap();
        for _ in 0..100 {
            drain_events(rt);
            if try_enumerate(rt) {
                rt.enumerated = true;
                log::info!("[USB-HID] keyboard enumerated slot={}", rt.slot);
                arm_hid_transfer(rt);
                break;
            }
            if rt.enum_gave_up {
                break;
            }
            crate::arch::spin_pause();
        }
        if !RUNTIME.as_ref().unwrap().enumerated {
            log::warn!("[USB-HID] keyboard enumeration pending (poll on vsync)");
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
                log::info!("[USB-HID] keyboard enumerated slot={}", rt.slot);
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
    let ptr = rt.cmd_phys + (rt.cmd_idx as u64) * 16;
    mmio::write64(op(rt), 0x18, ptr | rt.cmd_cycle as u64);
    crate::arch::memory_fence();
    let db = rt.bar_virt + rt.db_off as u64;
    mmio::write32(db, 0, 0);
}

unsafe fn wait_cmd(rt: &mut XhciRuntime) -> bool {
    for _ in 0..500_000 {
        if drain_events(rt) {
            return true;
        }
        crate::arch::spin_pause();
    }
    false
}

unsafe fn drain_events(rt: &mut XhciRuntime) -> bool {
    let mut cmd_done = false;
    loop {
        let i = (rt.evt_idx as usize) % EVT_TRBS;
        let dw3 = (*rt.evt.add(i)).dw3;
        if (dw3 & 1) != rt.evt_cycle {
            break;
        }
        let kind = dw3 & (0x3FF << 10);
        if kind == TRB_TYPE_EVT_CMD_COMP {
            cmd_done = true;
            let slot = (dw3 >> 24) & 0xFF;
            if rt.slot == 0 && slot != 0 {
                rt.slot = slot as u8;
                let dcbaa = p2v(rt.dcbaa_phys) as *mut DcbaaPage;
                (*dcbaa).ptrs[rt.slot as usize] = rt.dev_phys;
            }
        } else if kind == TRB_TYPE_EVT_TRANSFER {
            let report = core::slice::from_raw_parts(p2v(rt.hid_phys) as *const u8, HID_LEN);
            route_report(report, rt);
            rt.hid_armed = false;
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
    if report == &rt.last_hid {
        return;
    }
    let old = rt.last_hid;
    rt.last_hid.copy_from_slice(report);

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

unsafe fn write_input_address_ctx(in_phys: u64, port: u8) {
    let p = p2v(in_phys) as *mut u8;
    core::ptr::write_bytes(p, 0, 512);
    // Input control: add slot + EP0.
    core::ptr::write(p.add(4), 0x03);
    // Slot context @ 0x20 — 1 context entry, full-speed.
    core::ptr::write_unaligned(p.add(0x20) as *mut u32, (1 << 27) | (1 << 20));
    core::ptr::write_unaligned(p.add(0x24) as *mut u32, (port as u32) << 16);
    // EP0 context @ 0x40 — control (type 4), max packet 64.
    core::ptr::write_unaligned(p.add(0x44) as *mut u32, (4 << 3) | 64);
}

unsafe fn write_input_config_ep1(in_phys: u64, ep_ring_phys: u64) {
    let p = p2v(in_phys) as *mut u8;
    core::ptr::write_bytes(p, 0, 512);
    // Add EP1.
    core::ptr::write(p.add(4), 0x04);
    // EP1 context @ 0x60 — interrupt IN, max packet 8, interval 8.
    core::ptr::write_unaligned(p.add(0x64) as *mut u32, (7 << 3) | 8);
    core::ptr::write(p.add(0x62), 8);
    core::ptr::write_unaligned(p.add(0x68) as *mut u64, ep_ring_phys | 1);
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
        static ENABLE_FAIL: core::sync::atomic::AtomicBool =
            core::sync::atomic::AtomicBool::new(false);
        if !ENABLE_FAIL.swap(true, Ordering::Relaxed) {
            log::warn!("[USB-HID] enable slot failed slot={}", rt.slot);
        }
        rt.enum_gave_up = true;
        return false;
    }

    write_input_address_ctx(rt.in_phys, port);
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
    write_input_config_ep1(rt.in_phys, rt.ep_ring_phys);
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
    if rt.ep_idx as usize >= EP_TRBS {
        rt.ep_idx = 0;
        rt.ep_cycle ^= TRB_CYCLE;
    }
    rt.hid_armed = true;
    let db = rt.bar_virt + rt.db_off as u64 + (rt.slot as u64) * 4;
    mmio::write32(db, 0, 2);
}
