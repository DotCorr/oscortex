//! Phase 60 — Pseudo-terminal (pty/tty) layer.
//!
//! Provides POSIX-style pty pairs:
//!  - master fd: what the terminal emulator reads/writes
//!  - slave  fd: what shell/programs read/write
//!
//! Line discipline: echo on slave write, CR→LF translation.
//! TIOCGWINSZ ioctl returns a fixed 80×24 window size.
//!
//! Up to 8 concurrent pty pairs are supported.

use spin::Mutex;

const MAX_PTYS:    usize = 8;
const BUF_SIZE:    usize = 4096;

/// One direction of a pty ring buffer.
struct RingBuf {
    data: [u8; BUF_SIZE],
    head: usize,
    tail: usize,
}

impl RingBuf {
    const fn new() -> Self { Self { data: [0u8; BUF_SIZE], head: 0, tail: 0 } }

    fn is_empty(&self) -> bool { self.head == self.tail }

    fn available(&self) -> usize {
        (self.tail + BUF_SIZE - self.head) % BUF_SIZE
    }

    fn push(&mut self, b: u8) -> bool {
        let next = (self.tail + 1) % BUF_SIZE;
        if next == self.head { return false; } // full
        self.data[self.tail] = b;
        self.tail = next;
        true
    }

    fn pop(&mut self) -> Option<u8> {
        if self.is_empty() { return None; }
        let b = self.data[self.head];
        self.head = (self.head + 1) % BUF_SIZE;
        Some(b)
    }
}

struct PtyPair {
    /// Data written to master appears here for slave to read.
    master_to_slave: RingBuf,
    /// Data written to slave (with line discipline) appears here for master to read.
    slave_to_master: RingBuf,
    /// Is this slot allocated?
    alive: bool,
}

impl PtyPair {
    const fn new() -> Self {
        Self {
            master_to_slave: RingBuf::new(),
            slave_to_master: RingBuf::new(),
            alive: false,
        }
    }
}

static PTYS: Mutex<[PtyPair; MAX_PTYS]> = Mutex::new(
    [const { PtyPair::new() }; MAX_PTYS]
);

/// fd encoding: master fd = pty_idx * 2,  slave fd = pty_idx * 2 + 1.
/// The high byte of the fd is 0xAA to distinguish pty fds from regular fds.
const PTY_FD_TAG: u32 = 0xAA00_0000;

fn pty_idx_from_master(fd: u32) -> Option<usize> {
    if fd & 0xFF00_0000 != PTY_FD_TAG { return None; }
    let raw = (fd & 0x00FF_FFFF) as usize;
    if raw % 2 == 0 && raw / 2 < MAX_PTYS { Some(raw / 2) } else { None }
}
fn pty_idx_from_slave(fd: u32) -> Option<usize> {
    if fd & 0xFF00_0000 != PTY_FD_TAG { return None; }
    let raw = (fd & 0x00FF_FFFF) as usize;
    if raw % 2 == 1 && raw / 2 < MAX_PTYS { Some(raw / 2) } else { None }
}
fn master_fd(idx: usize) -> u32 { PTY_FD_TAG | (idx * 2) as u32 }
fn slave_fd(idx: usize)  -> u32 { PTY_FD_TAG | (idx * 2 + 1) as u32 }

/// Open a new pty pair. Returns (master_fd, slave_fd).
pub fn open() -> Result<(u32, u32), &'static str> {
    let mut ptys = PTYS.lock();
    for (idx, pty) in ptys.iter_mut().enumerate() {
        if !pty.alive {
            *pty = PtyPair::new();
            pty.alive = true;
            return Ok((master_fd(idx), slave_fd(idx)));
        }
    }
    Err("no free ptys")
}

/// Read from a pty fd. Master reads slave→master buffer; slave reads master→slave buffer.
pub fn read(fd: u32, buf: &mut [u8]) -> Result<usize, &'static str> {
    let mut ptys = PTYS.lock();
    if let Some(idx) = pty_idx_from_master(fd) {
        let pty = &mut ptys[idx];
        if !pty.alive { return Err("closed"); }
        let mut n = 0;
        while n < buf.len() {
            match pty.slave_to_master.pop() {
                Some(b) => { buf[n] = b; n += 1; }
                None => break,
            }
        }
        return Ok(n);
    }
    if let Some(idx) = pty_idx_from_slave(fd) {
        let pty = &mut ptys[idx];
        if !pty.alive { return Err("closed"); }
        let mut n = 0;
        while n < buf.len() {
            match pty.master_to_slave.pop() {
                Some(b) => { buf[n] = b; n += 1; }
                None => break,
            }
        }
        return Ok(n);
    }
    Err("invalid fd")
}

/// Write to a pty fd.
/// - Master write → pushes into master_to_slave (for slave to read).
/// - Slave write  → applies line discipline (echo, CR→LF) then pushes into slave_to_master.
pub fn write(fd: u32, data: &[u8]) -> Result<usize, &'static str> {
    let mut ptys = PTYS.lock();
    if let Some(idx) = pty_idx_from_master(fd) {
        let pty = &mut ptys[idx];
        if !pty.alive { return Err("closed"); }
        let mut n = 0;
        for &b in data {
            if pty.master_to_slave.push(b) { n += 1; } else { break; }
        }
        return Ok(n);
    }
    if let Some(idx) = pty_idx_from_slave(fd) {
        let pty = &mut ptys[idx];
        if !pty.alive { return Err("closed"); }
        let mut n = 0;
        for &b in data {
            // Line discipline: echo character back to master, CR→LF translation.
            let out = if b == b'\r' { b'\n' } else { b };
            // Echo to master.
            pty.slave_to_master.push(out);
            n += 1;
        }
        return Ok(n);
    }
    Err("invalid fd")
}

/// TIOCGWINSZ = 0x5413 — return packed { rows: u16, cols: u16, xpix: u16, ypix: u16 }.
pub fn ioctl(fd: u32, cmd: u64, arg: u64) -> i64 {
    const TIOCGWINSZ: u64 = 0x5413;
    const TIOCSWINSZ: u64 = 0x5414;

    // Validate fd belongs to an alive pty.
    let alive = {
        let ptys = PTYS.lock();
        pty_idx_from_master(fd).or_else(|| pty_idx_from_slave(fd))
            .map(|idx| ptys[idx].alive)
            .unwrap_or(false)
    };
    if !alive { return -9; } // EBADF

    match cmd {
        TIOCGWINSZ => {
            if arg == 0 { return -14; }
            // struct winsize { rows: u16, cols: u16, xpixel: u16, ypixel: u16 }
            let ws: [u16; 4] = [24, 80, 0, 0];
            unsafe {
                core::ptr::copy_nonoverlapping(
                    ws.as_ptr() as *const u8,
                    arg as *mut u8,
                    8,
                );
            }
            0
        }
        TIOCSWINSZ => 0, // accept but ignore
        _ => -25, // ENOTTY
    }
}
