//! OSCortex /init — first userspace process.
//!
//! Phases 43-46: Interactive shell with VFS, ramdisk, networking, and
//! full-screen surface support.
#![no_std]
#![no_main]
#![allow(dead_code)]

use core::arch::asm;

// ── Entry point ───────────────────────────────────────────────────────────────

core::arch::global_asm!(
    ".section .text._start",
    ".global _start",
    "_start:",
    "   xor  rbp, rbp",
    "   and  rsp, -16",
    "   call init_main",
    "0: hlt",
    "   jmp  0b",
);

// ── Syscall numbers ───────────────────────────────────────────────────────────

const SYS_EXIT:               u64 = 60;
const SYS_EXEC:               u64 = 59;
const SYS_WAITPID:            u64 = 61;
const SYS_FB_MAP:             u64 = 0x377;
const SYS_WM_NEXT_EVENT:      u64 = 0x378;
const SYS_VFS_LIST:           u64 = 0x379;
const SYS_VFS_READ:           u64 = 0x37A;
const SYS_VFS_WRITE:          u64 = 0x37B;
const SYS_VFS_STAT:           u64 = 0x37C;
const SYS_NET_INFO:           u64 = 0x37D;
const SYS_NET_SEND:           u64 = 0x37E;
const SYS_NET_RECV:           u64 = 0x37F;
const SYS_FB_RELEASE:         u64 = 0x380;
const SYS_SURFACE_FULLSCREEN: u64 = 0x381;
const SYS_EXEC_WAIT:          u64 = 0x39E;
const SYS_SERIAL_READ:        u64 = 0x383;
const SYS_SERIAL_WRITE:       u64 = 0x384;
const SYS_BLK_INFO:           u64 = 0x385;
const SYS_BLK_READ:           u64 = 0x386;
const SYS_BLK_WRITE:          u64 = 0x387;
const SYS_TCP_CONNECT:        u64 = 0x388;
const SYS_TCP_WRITE:          u64 = 0x389;
const SYS_TCP_READ:           u64 = 0x38A;
const SYS_TCP_CLOSE:          u64 = 0x38B;
const SYS_DHCP_DISCOVER:      u64 = 0x38C;
// Phase 51
const SYS_EXT2_MOUNT:         u64 = 0x38D;
const SYS_EXT2_LS:            u64 = 0x38E;
const SYS_EXT2_READ:          u64 = 0x38F;
// Phase 53
const SYS_SCHED_YIELD:        u64 = 0x390;
const SYS_GET_CPU_TIME:       u64 = 0x391;
// Phase 54
const SYS_FORK:               u64 = 0x392;
// Phase 55
const SYS_KILL_SIGNAL:        u64 = 0x393;
const SYS_SIGACTION:          u64 = 0x394;
const SYS_SIGRETURN:          u64 = 0x395;
// Phase 60
const SYS_PTY_OPEN:           u64 = 0x396;
const SYS_PTY_READ:           u64 = 0x397;
const SYS_PTY_WRITE:          u64 = 0x398;
const SYS_PTY_IOCTL:          u64 = 0x399;
// Phase 59
const SYS_NVME_INFO:          u64 = 0x39A;
const SYS_NVME_READ:          u64 = 0x39B;
const SYS_NVME_WRITE:         u64 = 0x39C;

// ── WM event kinds ────────────────────────────────────────────────────────────

const EV_KEY:     u32 = 3;

// ── ABI structs ───────────────────────────────────────────────────────────────

#[repr(C)]
struct FbInfo {
    addr:   u64,
    width:  u32,
    height: u32,
    pitch:  u32,
    bpp:    u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct WmEvent {
    seq:   u64,
    kind:  u32,
    flags: u32,
    a:     u64,
    b:     u64,
}

// ── Syscall helpers ───────────────────────────────────────────────────────────

// The kernel clobbers rdi, rsi, rdx, r8, r9, r10 during dispatch (rearranges
// args into SysV order then calls a Rust function). Declare them all as
// inlateout/lateout so the compiler reloads any values it needs after the call.
#[inline(always)]
unsafe fn syscall1(nr: u64, a0: u64) -> i64 {
    let ret: i64;
    asm!("syscall",
        inlateout("rax") nr  => ret,
        inlateout("rdi") a0  => _,
        lateout("rcx") _, lateout("r11") _,
        lateout("rsi") _, lateout("rdx") _,
        lateout("r8")  _, lateout("r9")  _, lateout("r10") _,
        options(nostack, preserves_flags));
    ret
}

#[inline(always)]
unsafe fn syscall2(nr: u64, a0: u64, a1: u64) -> i64 {
    let ret: i64;
    asm!("syscall",
        inlateout("rax") nr  => ret,
        inlateout("rdi") a0  => _,
        inlateout("rsi") a1  => _,
        lateout("rcx") _, lateout("r11") _,
        lateout("rdx") _, lateout("r8")  _, lateout("r9")  _, lateout("r10") _,
        options(nostack, preserves_flags));
    ret
}

#[inline(always)]
unsafe fn syscall4(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: i64;
    asm!("syscall",
        inlateout("rax") nr  => ret,
        inlateout("rdi") a0  => _,
        inlateout("rsi") a1  => _,
        inlateout("rdx") a2  => _,
        inlateout("r10") a3  => _,
        lateout("rcx") _, lateout("r11") _,
        lateout("r8")  _, lateout("r9")  _,
        options(nostack, preserves_flags));
    ret
}

/// Convenience macro matching the call-sites that pass 4 args.
macro_rules! syscall {
    ($nr:expr, $a0:expr, $a1:expr, $a2:expr, $a3:expr) => {
        unsafe { syscall4($nr, $a0 as u64, $a1 as u64, $a2 as u64, $a3 as u64) }
    };
    ($nr:expr, $a0:expr, $a1:expr, $a2:expr) => {
        unsafe { crate::syscall4($nr, $a0 as u64, $a1 as u64, $a2 as u64, 0) }
    };
    ($nr:expr, $a0:expr) => {
        unsafe { syscall1($nr, $a0 as u64) }
    };
    ($nr:expr) => {
        unsafe { syscall1($nr, 0) }
    };
}

fn fb_map(info: &mut FbInfo) -> i64 {
    unsafe { syscall1(SYS_FB_MAP, info as *mut FbInfo as u64) }
}
fn wm_next_event(ev: &mut WmEvent) -> i64 {
    unsafe { syscall1(SYS_WM_NEXT_EVENT, ev as *mut WmEvent as u64) }
}
fn sys_exit(code: u64) -> ! {
    unsafe { syscall1(SYS_EXIT, code); }
    loop { unsafe { asm!("hlt", options(nostack)); } }
}
fn vfs_list(path: &[u8], buf: &mut [u8]) -> i64 {
    unsafe { syscall4(SYS_VFS_LIST,
        path.as_ptr() as u64, path.len() as u64,
        buf.as_mut_ptr() as u64, buf.len() as u64) }
}
fn vfs_read(path: &[u8], buf: &mut [u8]) -> i64 {
    unsafe { syscall4(SYS_VFS_READ,
        path.as_ptr() as u64, path.len() as u64,
        buf.as_mut_ptr() as u64, buf.len() as u64) }
}
fn vfs_write(path: &[u8], data: &[u8]) -> i64 {
    unsafe { syscall4(SYS_VFS_WRITE,
        path.as_ptr() as u64, path.len() as u64,
        data.as_ptr() as u64, data.len() as u64) }
}
fn net_info(buf: &mut [u8]) -> i64 {
    unsafe { syscall2(SYS_NET_INFO, buf.as_mut_ptr() as u64, buf.len() as u64) }
}
fn sys_exec(path: &[u8]) -> i64 {
    unsafe { syscall2(SYS_EXEC, path.as_ptr() as u64, path.len() as u64) }
}
fn sys_waitpid(pid: u64) -> i64 {
    unsafe { syscall1(SYS_WAITPID, pid) }
}

// ── Pixel ops ─────────────────────────────────────────────────────────────────

#[inline(always)]
unsafe fn poke_px(base: u64, pitch: u32, x: u32, y: u32, color: u32) {
    let off = y as u64 * pitch as u64 + x as u64 * 4;
    ((base + off) as *mut u32).write_volatile(color);
}

fn fill_rect(fb: &FbInfo, x: u32, y: u32, w: u32, h: u32, color: u32) {
    let x1 = (x + w).min(fb.width);
    let y1 = (y + h).min(fb.height);
    for row in y..y1 {
        for col in x..x1 {
            unsafe { poke_px(fb.addr, fb.pitch, col, row, color); }
        }
    }
}

// ── IBM PC 8×8 bitmap font ────────────────────────────────────────────────────

#[rustfmt::skip]
static FONT: [[u8; 8]; 96] = [
/*0x20*/ [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
/*0x21*/ [0x18,0x3C,0x3C,0x18,0x18,0x00,0x18,0x00],
/*0x22*/ [0x66,0x66,0x24,0x00,0x00,0x00,0x00,0x00],
/*0x23*/ [0x6C,0x6C,0xFE,0x6C,0xFE,0x6C,0x6C,0x00],
/*0x24*/ [0x18,0x3E,0x60,0x3C,0x06,0x7C,0x18,0x00],
/*0x25*/ [0x00,0xC6,0xCC,0x18,0x30,0x66,0xC6,0x00],
/*0x26*/ [0x38,0x6C,0x38,0x76,0xDC,0xCC,0x76,0x00],
/*0x27*/ [0x18,0x18,0x30,0x00,0x00,0x00,0x00,0x00],
/*0x28*/ [0x0C,0x18,0x30,0x30,0x30,0x18,0x0C,0x00],
/*0x29*/ [0x30,0x18,0x0C,0x0C,0x0C,0x18,0x30,0x00],
/*0x2A*/ [0x00,0x66,0x3C,0xFF,0x3C,0x66,0x00,0x00],
/*0x2B*/ [0x00,0x18,0x18,0x7E,0x18,0x18,0x00,0x00],
/*0x2C*/ [0x00,0x00,0x00,0x00,0x00,0x18,0x18,0x30],
/*0x2D*/ [0x00,0x00,0x00,0x7E,0x00,0x00,0x00,0x00],
/*0x2E*/ [0x00,0x00,0x00,0x00,0x00,0x18,0x18,0x00],
/*0x2F*/ [0x06,0x0C,0x18,0x30,0x60,0xC0,0x80,0x00],
/*0x30*/ [0x3C,0x66,0x6E,0x76,0x66,0x66,0x3C,0x00],
/*0x31*/ [0x18,0x38,0x18,0x18,0x18,0x18,0x7E,0x00],
/*0x32*/ [0x3C,0x66,0x06,0x1C,0x30,0x66,0x7E,0x00],
/*0x33*/ [0x3C,0x66,0x06,0x1C,0x06,0x66,0x3C,0x00],
/*0x34*/ [0x06,0x1E,0x36,0x66,0x7F,0x06,0x06,0x00],
/*0x35*/ [0x7E,0x60,0x7C,0x06,0x06,0x66,0x3C,0x00],
/*0x36*/ [0x1C,0x30,0x60,0x7C,0x66,0x66,0x3C,0x00],
/*0x37*/ [0x7E,0x66,0x0C,0x18,0x30,0x30,0x30,0x00],
/*0x38*/ [0x3C,0x66,0x66,0x3C,0x66,0x66,0x3C,0x00],
/*0x39*/ [0x3C,0x66,0x66,0x3E,0x06,0x0C,0x38,0x00],
/*0x3A*/ [0x00,0x18,0x18,0x00,0x00,0x18,0x18,0x00],
/*0x3B*/ [0x00,0x18,0x18,0x00,0x00,0x18,0x18,0x30],
/*0x3C*/ [0x06,0x0C,0x18,0x30,0x18,0x0C,0x06,0x00],
/*0x3D*/ [0x00,0x00,0x7E,0x00,0x00,0x7E,0x00,0x00],
/*0x3E*/ [0x60,0x30,0x18,0x0C,0x18,0x30,0x60,0x00],
/*0x3F*/ [0x3C,0x66,0x06,0x0C,0x18,0x00,0x18,0x00],
/*0x40*/ [0x3E,0x63,0x6F,0x69,0x6F,0x60,0x3E,0x00],
/*0x41*/ [0x18,0x3C,0x66,0x7E,0x66,0x66,0x66,0x00],
/*0x42*/ [0x7C,0x66,0x66,0x7C,0x66,0x66,0x7C,0x00],
/*0x43*/ [0x3C,0x66,0x60,0x60,0x60,0x66,0x3C,0x00],
/*0x44*/ [0x78,0x6C,0x66,0x66,0x66,0x6C,0x78,0x00],
/*0x45*/ [0x7E,0x60,0x60,0x78,0x60,0x60,0x7E,0x00],
/*0x46*/ [0x7E,0x60,0x60,0x78,0x60,0x60,0x60,0x00],
/*0x47*/ [0x3C,0x66,0x60,0x6E,0x66,0x66,0x3C,0x00],
/*0x48*/ [0x66,0x66,0x66,0x7E,0x66,0x66,0x66,0x00],
/*0x49*/ [0x3C,0x18,0x18,0x18,0x18,0x18,0x3C,0x00],
/*0x4A*/ [0x1E,0x0C,0x0C,0x0C,0x0C,0x6C,0x38,0x00],
/*0x4B*/ [0x66,0x6C,0x78,0x70,0x78,0x6C,0x66,0x00],
/*0x4C*/ [0x60,0x60,0x60,0x60,0x60,0x60,0x7E,0x00],
/*0x4D*/ [0x63,0x77,0x7F,0x6B,0x63,0x63,0x63,0x00],
/*0x4E*/ [0x66,0x76,0x7E,0x7E,0x6E,0x66,0x66,0x00],
/*0x4F*/ [0x3C,0x66,0x66,0x66,0x66,0x66,0x3C,0x00],
/*0x50*/ [0x7C,0x66,0x66,0x7C,0x60,0x60,0x60,0x00],
/*0x51*/ [0x3C,0x66,0x66,0x66,0x66,0x3C,0x1E,0x00],
/*0x52*/ [0x7C,0x66,0x66,0x7C,0x78,0x6C,0x66,0x00],
/*0x53*/ [0x3C,0x66,0x60,0x3C,0x06,0x66,0x3C,0x00],
/*0x54*/ [0x7E,0x18,0x18,0x18,0x18,0x18,0x18,0x00],
/*0x55*/ [0x66,0x66,0x66,0x66,0x66,0x66,0x3C,0x00],
/*0x56*/ [0x66,0x66,0x66,0x66,0x66,0x3C,0x18,0x00],
/*0x57*/ [0x63,0x63,0x63,0x6B,0x7F,0x77,0x63,0x00],
/*0x58*/ [0x66,0x66,0x3C,0x18,0x3C,0x66,0x66,0x00],
/*0x59*/ [0x66,0x66,0x66,0x3C,0x18,0x18,0x18,0x00],
/*0x5A*/ [0x7E,0x06,0x0C,0x18,0x30,0x60,0x7E,0x00],
/*0x5B*/ [0x3C,0x30,0x30,0x30,0x30,0x30,0x3C,0x00],
/*0x5C*/ [0xC0,0x60,0x30,0x18,0x0C,0x06,0x02,0x00],
/*0x5D*/ [0x3C,0x0C,0x0C,0x0C,0x0C,0x0C,0x3C,0x00],
/*0x5E*/ [0x10,0x38,0x6C,0xC6,0x00,0x00,0x00,0x00],
/*0x5F*/ [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0xFF],
/*0x60*/ [0x18,0x18,0x0C,0x00,0x00,0x00,0x00,0x00],
/*0x61*/ [0x00,0x00,0x3C,0x06,0x3E,0x66,0x3E,0x00],
/*0x62*/ [0x60,0x60,0x7C,0x66,0x66,0x66,0x7C,0x00],
/*0x63*/ [0x00,0x00,0x3C,0x60,0x60,0x60,0x3C,0x00],
/*0x64*/ [0x06,0x06,0x3E,0x66,0x66,0x66,0x3E,0x00],
/*0x65*/ [0x00,0x00,0x3C,0x66,0x7E,0x60,0x3C,0x00],
/*0x66*/ [0x1C,0x30,0x7C,0x30,0x30,0x30,0x30,0x00],
/*0x67*/ [0x00,0x00,0x3E,0x66,0x66,0x3E,0x06,0x7C],
/*0x68*/ [0x60,0x60,0x7C,0x66,0x66,0x66,0x66,0x00],
/*0x69*/ [0x18,0x00,0x38,0x18,0x18,0x18,0x3C,0x00],
/*0x6A*/ [0x06,0x00,0x0E,0x06,0x06,0x06,0x66,0x3C],
/*0x6B*/ [0x60,0x60,0x66,0x6C,0x78,0x6C,0x66,0x00],
/*0x6C*/ [0x38,0x18,0x18,0x18,0x18,0x18,0x3C,0x00],
/*0x6D*/ [0x00,0x00,0x66,0x7F,0x7F,0x6B,0x63,0x00],
/*0x6E*/ [0x00,0x00,0x7C,0x66,0x66,0x66,0x66,0x00],
/*0x6F*/ [0x00,0x00,0x3C,0x66,0x66,0x66,0x3C,0x00],
/*0x70*/ [0x00,0x00,0x7C,0x66,0x66,0x7C,0x60,0x60],
/*0x71*/ [0x00,0x00,0x3E,0x66,0x66,0x3E,0x06,0x06],
/*0x72*/ [0x00,0x00,0x7C,0x66,0x60,0x60,0x60,0x00],
/*0x73*/ [0x00,0x00,0x3E,0x60,0x3C,0x06,0x7C,0x00],
/*0x74*/ [0x30,0x30,0x7C,0x30,0x30,0x30,0x1C,0x00],
/*0x75*/ [0x00,0x00,0x66,0x66,0x66,0x66,0x3E,0x00],
/*0x76*/ [0x00,0x00,0x66,0x66,0x66,0x3C,0x18,0x00],
/*0x77*/ [0x00,0x00,0x63,0x6B,0x7F,0x3E,0x36,0x00],
/*0x78*/ [0x00,0x00,0x66,0x3C,0x18,0x3C,0x66,0x00],
/*0x79*/ [0x00,0x00,0x66,0x66,0x66,0x3E,0x06,0x7C],
/*0x7A*/ [0x00,0x00,0x7E,0x0C,0x18,0x30,0x7E,0x00],
/*0x7B*/ [0x0E,0x18,0x18,0x70,0x18,0x18,0x0E,0x00],
/*0x7C*/ [0x18,0x18,0x18,0x00,0x18,0x18,0x18,0x00],
/*0x7D*/ [0x70,0x18,0x18,0x0E,0x18,0x18,0x70,0x00],
/*0x7E*/ [0x76,0xDC,0x00,0x00,0x00,0x00,0x00,0x00],
/*0x7F*/ [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
];

fn draw_char(fb: &FbInfo, x: u32, y: u32, ch: u8, fg: u32, bg: u32) {
    let idx = ch.wrapping_sub(0x20) as usize;
    if idx >= 96 { return; }
    let glyph = &FONT[idx];
    for row in 0..8u32 {
        if y + row >= fb.height { break; }
        let bits = glyph[row as usize];
        for col in 0..8u32 {
            if x + col >= fb.width { break; }
            let color = if bits & (0x80 >> col) != 0 { fg } else { bg };
            unsafe { poke_px(fb.addr, fb.pitch, x + col, y + row, color); }
        }
    }
}

fn draw_str(fb: &FbInfo, x: u32, y: u32, s: &[u8], fg: u32, bg: u32) {
    for (i, &ch) in s.iter().enumerate() {
        draw_char(fb, x + i as u32 * 8, y, ch, fg, bg);
    }
}

fn draw_str_at(fb: &FbInfo, x: &mut u32, y: u32, s: &[u8], fg: u32, bg: u32) {
    for &ch in s {
        draw_char(fb, *x, y, ch, fg, bg);
        *x += 8;
    }
}

// ── Keyboard scancode → ASCII ─────────────────────────────────────────────────

fn scancode_to_ascii(sc: u8, shift: bool) -> u8 {
    const NORM: [u8; 58] = [
        0,  0x1B, b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0',
        b'-', b'=', 0x08, b'\t', b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i',
        b'o', b'p', b'[', b']', b'\n', 0, b'a', b's', b'd', b'f', b'g', b'h',
        b'j', b'k', b'l', b';', b'\'', b'`', 0, b'\\', b'z', b'x', b'c', b'v',
        b'b', b'n', b'm', b',', b'.', b'/', 0, b'*', 0, b' ',
    ];
    const SHFT: [u8; 58] = [
        0,  0x1B, b'!', b'@', b'#', b'$', b'%', b'^', b'&', b'*', b'(', b')',
        b'_', b'+', 0x08, b'\t', b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I',
        b'O', b'P', b'{', b'}', b'\n', 0, b'A', b'S', b'D', b'F', b'G', b'H',
        b'J', b'K', b'L', b':', b'"', b'~', 0, b'|', b'Z', b'X', b'C', b'V',
        b'B', b'N', b'M', b'<', b'>', b'?', 0, b'*', 0, b' ',
    ];
    if sc as usize >= 58 { return 0; }
    if shift { SHFT[sc as usize] } else { NORM[sc as usize] }
}

// ── Colours ───────────────────────────────────────────────────────────────────

const C_BG:     u32 = 0x0D1117;
const C_PANEL:  u32 = 0x161B22;
const C_BORDER: u32 = 0x30363D;
const C_GREEN:  u32 = 0x3FB950;
const C_CYAN:   u32 = 0x79C0FF;
const C_WHITE:  u32 = 0xE6EDF3;
const C_YELLOW: u32 = 0xD29922;
const C_DIM:    u32 = 0x8B949E;
const C_RED:    u32 = 0xF85149;

// ── Shell state ───────────────────────────────────────────────────────────────

struct Shell<'a> {
    fb:       &'a FbInfo,
    // Terminal region
    tx0: u32,  ty0: u32,       // top-left of text area
    tw:  u32,  th:  u32,       // width + height in pixels
    cx:  u32,  cy:  u32,       // current cursor pixel position
    // Input buffer
    ibuf:  [u8; 256],
    ilen:  usize,
    shift: bool,
}

impl<'a> Shell<'a> {
    fn new(fb: &'a FbInfo, tx0: u32, ty0: u32, tw: u32, th: u32) -> Self {
        Shell { fb, tx0, ty0, tw, th, cx: tx0, cy: ty0,
                ibuf: [0u8; 256], ilen: 0, shift: false }
    }

    fn newline(&mut self) {
        self.cx  = self.tx0;
        self.cy += 12;
        if self.cy + 12 > self.ty0 + self.th {
            // Scroll: clear and restart.
            fill_rect(self.fb, self.tx0, self.ty0, self.tw, self.th, C_PANEL);
            self.cy = self.ty0;
        }
    }

    fn putch(&mut self, ch: u8, fg: u32) {
        if ch == b'\n' { self.newline(); return; }
        if self.cx + 8 > self.tx0 + self.tw { self.newline(); }
        draw_char(self.fb, self.cx, self.cy, ch, fg, C_PANEL);
        self.cx += 8;
    }

    fn puts(&mut self, s: &[u8], fg: u32) {
        for &ch in s { self.putch(ch, fg); }
    }

    fn put_i64(&mut self, mut v: i64, fg: u32) {
        if v < 0 { self.putch(b'-', fg); v = -v; }
        self.put_u64(v as u64, fg);
    }
    fn put_u64(&mut self, v: u64, fg: u32) {
        if v == 0 { self.putch(b'0', fg); return; }
        let mut buf = [0u8; 20];
        let mut n = 0usize;
        let mut x = v;
        while x > 0 { buf[n] = b'0' + (x % 10) as u8; x /= 10; n += 1; }
        for i in (0..n).rev() { self.putch(buf[i], fg); }
    }

    fn prompt(&mut self) {
        self.newline();
        self.puts(b"$ ", C_YELLOW);
        self.ilen = 0;
    }

    fn cursor_show(&self, on: bool) {
        let col = if on { C_CYAN } else { C_PANEL };
        fill_rect(self.fb, self.cx, self.cy, 8, 8, col);
    }

    /// Run the entered command (ibuf[..ilen]).
    fn run_command(&mut self) {
        if self.ilen == 0 { return; }
        // Copy command to a local buffer to avoid self-borrow issues.
        let mut lcmd = [0u8; 256];
        let llen = self.ilen;
        lcmd[..llen].copy_from_slice(&self.ibuf[..llen]);
        let cmd = &lcmd[..llen];

        // Split on first space to get verb and rest.
        let (verb, rest) = match cmd.iter().position(|&b| b == b' ') {
            Some(i) => (&cmd[..i], cmd[i + 1..].trim_ascii_start()),
            None    => (cmd, b"" as &[u8]),
        };

        self.newline();
        match verb {
            b"help" => {
                self.puts(b"commands: help clear ls cat write ps exec serial blk ping net", C_GREEN);
                self.newline();
                self.puts(b"  mount lse2 cate2 nvme pty yield cputime exit", C_GREEN);
            }
            b"clear" => {
                fill_rect(self.fb, self.tx0, self.ty0, self.tw, self.th, C_PANEL);
                self.cx = self.tx0;
                self.cy = self.ty0;
            }
            b"ls" => {
                let path = if rest.is_empty() { b"/" as &[u8] } else { rest };
                let mut buf = [0u8; 4096];
                let n = vfs_list(path, &mut buf);
                if n < 0 { self.puts(b"error", C_RED); }
                else {
                    let out = &buf[..n as usize];
                    for line in out.split(|&b| b == b'\n') {
                        if line.is_empty() { continue; }
                        self.newline();
                        self.puts(line, C_WHITE);
                    }
                }
            }
            b"cat" => {
                if rest.is_empty() { self.puts(b"usage: cat <path>", C_RED); }
                else {
                    let mut buf = [0u8; 4096];
                    let n = vfs_read(rest, &mut buf);
                    if n < 0 { self.puts(b"not found", C_RED); }
                    else { self.puts(&buf[..n as usize], C_WHITE); }
                }
            }
            b"write" => {
                // write <path> <text>
                let (wpath, wdata) = match rest.iter().position(|&b| b == b' ') {
                    Some(i) => (&rest[..i], &rest[i + 1..]),
                    None    => { self.puts(b"usage: write <path> <text>", C_RED); return; }
                };
                let r = vfs_write(wpath, wdata);
                if r < 0 { self.puts(b"error: ", C_RED); self.put_i64(r, C_RED); }
                else { self.puts(b"written", C_GREEN); }
            }
            b"ps" => {
                let mut buf = [0u8; 4096];
                let n = vfs_read(b"/sys/process_list", &mut buf);
                if n < 0 { self.puts(b"unavailable", C_DIM); }
                else { self.puts(&buf[..n as usize], C_WHITE); }
            }
            b"exec" => {
                if rest.is_empty() { self.puts(b"usage: exec <path>", C_RED); }
                else {
                    // Phase 47: sys_exec_wait blocks until child exits and returns exit code.
                    let exit = unsafe {
                        let mut ret: i64;
                        core::arch::asm!(
                            "mov r10, rcx; syscall",
                            in("rax") SYS_EXEC_WAIT,
                            in("rdi") rest.as_ptr() as u64,
                            in("rsi") rest.len() as u64,
                            lateout("rax") ret,
                            out("rcx") _, out("r11") _, out("r10") _,
                            options(nostack)
                        );
                        ret
                    };
                    if exit < 0 {
                        self.puts(b"exec failed: ", C_RED); self.put_i64(exit, C_RED);
                    } else {
                        self.puts(b"exit=", C_DIM); self.put_i64(exit, C_DIM);
                    }
                }
            }
            b"serial" => {
                // Phase 48: drain COM1 and echo to screen.
                let mut buf = [0u8; 64];
                let n = unsafe {
                    let mut ret: i64;
                    core::arch::asm!(
                        "mov r10, rcx; syscall",
                        in("rax") SYS_SERIAL_READ,
                        in("rdi") buf.as_mut_ptr() as u64,
                        in("rsi") buf.len() as u64,
                        lateout("rax") ret,
                        out("rcx") _, out("r11") _, out("r10") _,
                        options(nostack)
                    );
                    ret
                };
                if n > 0 { self.puts(&buf[..n as usize], C_WHITE); }
                else { self.puts(b"(no serial data)", C_DIM); }
            }
            b"blk" => {
                // Phase 49: show block device info.
                let mut buf = [0u8; 128];
                let ret = unsafe {
                    let mut r: i64;
                    core::arch::asm!(
                        "mov r10, rcx; syscall",
                        in("rax") SYS_BLK_INFO,
                        in("rdi") buf.as_mut_ptr() as u64,
                        lateout("rax") r,
                        out("rcx") _, out("r11") _, out("r10") _,
                        options(nostack)
                    );
                    r
                };
                if ret >= 0 {
                    let n = buf.iter().position(|&b| b == 0).unwrap_or(128);
                    if n > 0 { self.puts(&buf[..n], C_WHITE); }
                    else { self.puts(b"(no blk info)", C_DIM); }
                } else {
                    self.puts(b"blk: no device", C_RED);
                }
            }
            b"ping" => {
                // Phase 50: open TCP connection to 10.0.2.2:80, send a GET, read response.
                // 10.0.2.2 in big-endian = 0x0202000A
                let ip_be: u32 = u32::from_be_bytes([10, 0, 2, 2]);
                let fd = unsafe {
                    let mut r: i64;
                    core::arch::asm!(
                        "mov r10, rcx; syscall",
                        in("rax") SYS_TCP_CONNECT,
                        in("rdi") ip_be as u64,
                        in("rsi") 80u64,
                        lateout("rax") r,
                        out("rcx") _, out("r11") _, out("r10") _,
                        options(nostack)
                    );
                    r
                };
                if fd < 0 {
                    self.puts(b"tcp_connect failed: ", C_RED); self.put_i64(fd, C_RED);
                } else {
                    let req = b"GET / HTTP/1.0\r\nHost: 10.0.2.2\r\n\r\n";
                    let _w = unsafe {
                        let mut r: i64;
                        core::arch::asm!(
                            "mov r10, rcx; syscall",
                            in("rax") SYS_TCP_WRITE,
                            in("rdi") fd as u64,
                            in("rsi") req.as_ptr() as u64,
                            in("rdx") req.len() as u64,
                            lateout("rax") r,
                            out("rcx") _, out("r11") _, out("r10") _,
                            options(nostack)
                        );
                        r
                    };
                    let mut rbuf = [0u8; 128];
                    let rn = unsafe {
                        let mut r: i64;
                        core::arch::asm!(
                            "mov r10, rcx; syscall",
                            in("rax") SYS_TCP_READ,
                            in("rdi") fd as u64,
                            in("rsi") rbuf.as_mut_ptr() as u64,
                            in("rdx") rbuf.len() as u64,
                            lateout("rax") r,
                            out("rcx") _, out("r11") _, out("r10") _,
                            options(nostack)
                        );
                        r
                    };
                    if rn > 0 { self.puts(&rbuf[..rn as usize], C_WHITE); }
                    else { self.puts(b"(no response)", C_DIM); }
                    let _ = unsafe {
                        let mut r: i64;
                        core::arch::asm!(
                            "mov r10, rcx; syscall",
                            in("rax") SYS_TCP_CLOSE,
                            in("rdi") fd as u64,
                            lateout("rax") r,
                            out("rcx") _, out("r11") _, out("r10") _,
                            options(nostack)
                        );
                        r
                    };
                }
            }
            b"net" => {
                let mut buf = [0u8; 128];
                let n = net_info(&mut buf);
                if n <= 0 { self.puts(b"no network device", C_DIM); }
                else { self.puts(&buf[..n as usize], C_WHITE); }
            }
            b"mount" => {
                let r = syscall!(SYS_EXT2_MOUNT, 0u64, 0u64, 0u64, 0u64);
                if r == 0 { self.puts(b"ext2 mounted", C_GREEN); }
                else { self.puts(b"ext2 mount failed", C_RED); }
            }
            b"lse2" => {
                let mut buf = [0u8; 512];
                let path = b"/";
                let n = syscall!(SYS_EXT2_LS,
                    path.as_ptr() as u64, path.len() as u64,
                    buf.as_mut_ptr() as u64, buf.len() as u64);
                if n > 0 { self.puts(&buf[..n as usize], C_WHITE); }
                else { self.puts(b"(empty or not mounted)", C_DIM); }
            }
            b"cate2" => {
                let mut buf = [0u8; 4096];
                let path = if rest.is_empty() { b"/etc/os-release" as &[u8] } else { rest };
                let n = syscall!(SYS_EXT2_READ,
                    path.as_ptr() as u64, path.len() as u64,
                    buf.as_mut_ptr() as u64, buf.len() as u64);
                if n > 0 { self.puts(&buf[..n as usize], C_WHITE); }
                else { self.puts(b"not found", C_DIM); }
            }
            b"nvme" => {
                let mut buf = [0u8; 128];
                let n = syscall!(SYS_NVME_INFO,
                    buf.as_mut_ptr() as u64, buf.len() as u64, 0u64, 0u64);
                if n > 0 { self.puts(&buf[..n as usize], C_WHITE); }
                else { self.puts(b"nvme: no info", C_DIM); }
            }
            b"pty" => {
                let r = syscall!(SYS_PTY_OPEN, 0u64, 0u64, 0u64, 0u64);
                let master = (r >> 32) as u32;
                let slave  = (r & 0xFFFF_FFFF) as u32;
                self.puts(b"pty master_fd=", C_GREEN);
                self.put_u64(master as u64, C_WHITE);
                self.puts(b" slave_fd=", C_GREEN);
                self.put_u64(slave as u64, C_WHITE);
            }
            b"yield" => {
                syscall!(SYS_SCHED_YIELD, 0u64, 0u64, 0u64, 0u64);
                self.puts(b"yielded", C_DIM);
            }
            b"cputime" => {
                let t = syscall!(SYS_GET_CPU_TIME, 0u64, 0u64, 0u64, 0u64);
                self.puts(b"cpu_ticks=", C_GREEN);
                self.put_u64(t as u64, C_WHITE);
            }
            b"exit" => { sys_exit(0); }
            other => {
                self.puts(b"unknown: ", C_RED);
                self.puts(other, C_RED);
            }
        }
    }
}

trait TrimAscii {
    fn trim_ascii_start(&self) -> &Self;
}
impl TrimAscii for [u8] {
    fn trim_ascii_start(&self) -> &[u8] {
        let mut i = 0;
        while i < self.len() && self[i] == b' ' { i += 1; }
        &self[i..]
    }
}

// ── UI ────────────────────────────────────────────────────────────────────────

fn draw_frame(fb: &FbInfo) {
    fill_rect(fb, 0, 0, fb.width, fb.height, C_BG);
    fill_rect(fb, 0, 0, fb.width, 28, C_PANEL);
    fill_rect(fb, 0, 27, fb.width, 1, C_BORDER);
    draw_str(fb, 10, 10, b"OSCortex Shell  [Phase 51-60]", C_GREEN, C_PANEL);
    draw_str(fb, fb.width.saturating_sub(200), 10, b"type 'help'", C_DIM, C_PANEL);

    let px = 8u32; let py = 34u32;
    let pw = fb.width - 16;
    let ph = fb.height - 42;
    fill_rect(fb, px, py, pw, ph, C_PANEL);
    fill_rect(fb, px, py, pw, 1, C_BORDER);
    fill_rect(fb, px, py + ph - 1, pw, 1, C_BORDER);
    fill_rect(fb, px, py, 1, ph, C_BORDER);
    fill_rect(fb, px + pw - 1, py, 1, ph, C_BORDER);
}

// ── Main ─────────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn init_main() -> ! {
    let mut fb = FbInfo { addr: 0, width: 0, height: 0, pitch: 0, bpp: 0 };
    let ret = fb_map(&mut fb);
    if ret < 0 || fb.addr == 0 {
        loop { unsafe { asm!("hlt", options(nostack)); } }
    }

    draw_frame(&fb);

    let px = 10u32; let py = 36u32;
    let pw = fb.width - 20;
    let ph = fb.height - 44;

    let mut sh = Shell::new(&fb, px + 4, py + 4, pw - 8, ph - 8);
    sh.puts(b"OSCortex interactive shell", C_GREEN);
    sh.prompt();
    sh.cursor_show(true);

    let mut ev = WmEvent::default();
    let mut spin: u64 = 0;

    loop {
        let r = wm_next_event(&mut ev);
        if r >= 0 && ev.kind == EV_KEY {
            spin = 0;
            let pressed = ev.flags != 0;
            let sc = ev.a as u8;
            if sc >= 0x80 { continue; }
            if sc == 0x2A || sc == 0x36 { sh.shift = pressed; continue; }
            if !pressed { continue; }

            let ch = scancode_to_ascii(sc, sh.shift);
            if ch == 0 { continue; }

            sh.cursor_show(false);

            match ch {
                b'\n' | b'\r' => {
                    sh.run_command();
                    sh.prompt();
                }
                0x08 => {
                    if sh.ilen > 0 {
                        sh.ilen -= 1;
                        if sh.cx >= sh.tx0 + 8 {
                            sh.cx -= 8;
                            fill_rect(sh.fb, sh.cx, sh.cy, 8, 8, C_PANEL);
                        }
                    }
                }
                _ => {
                    if sh.ilen < 255 {
                        sh.ibuf[sh.ilen] = ch;
                        sh.ilen += 1;
                        sh.putch(ch, C_WHITE);
                    }
                }
            }
            sh.cursor_show(true);
        } else {
            spin = spin.wrapping_add(1);
            for _ in 0..200u32 { unsafe { asm!("pause", options(nostack)); } }
            if spin & 0x1FFF == 0 {
                let on = (spin >> 13) & 1 == 0;
                sh.cursor_show(on);
            }
        }
    }
}

// ── Panic ─────────────────────────────────────────────────────────────────────

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop { unsafe { asm!("hlt", options(nostack)); } }
}

