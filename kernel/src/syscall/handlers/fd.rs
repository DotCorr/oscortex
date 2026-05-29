use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use crate::syscall::poll::{
    alloc_synth_fd, epoll_wake, eventfd_write,
    sys_epoll_create_real, sys_epoll_ctl_real, sys_epoll_wait_real, sys_eventfd2,
    sys_timerfd_create_real, sys_timerfd_settime_real, EVENTFD_TABLE, TimerState,
    TIMERFD_TABLE,
};
use crate::syscall::state::FUTEX_WAITERS;
use crate::syscall::tables::{OpenFile, PipeBuf, MAX_OPEN_FILES, OPEN_FILES, PIPE_BUF_SIZE, PIPES};
use crate::syscall::poll::monotonic_ns;

/// Allocate an fd (≥3) backed by `data` from the VFS.
fn fd_alloc(data: &'static [u8]) -> Option<u64> {
    let mut tbl = OPEN_FILES.lock();
    for (i, slot) in tbl.iter_mut().enumerate().skip(3) {
        if !slot.used {
            *slot = OpenFile { data, offset: 0, used: true, is_dir: false, dir_path: [0; 192], dir_path_len: 0, pipe_id: -1, pipe_is_write: false };
            return Some(i as u64);
        }
    }
    None
}

/// Allocate an fd that represents an open directory (no data, no read).
/// `path` is the absolute path of the directory, stored for openat() resolution.
fn fd_alloc_dir(path: &str) -> Option<u64> {
    let mut tbl = OPEN_FILES.lock();
    for (i, slot) in tbl.iter_mut().enumerate().skip(3) {
        if !slot.used {
            let bytes = path.as_bytes();
            let n = bytes.len().min(192);
            let mut buf = [0u8; 192];
            buf[..n].copy_from_slice(&bytes[..n]);
            *slot = OpenFile { data: &[], offset: 0, used: true, is_dir: true, dir_path: buf, dir_path_len: n, pipe_id: -1, pipe_is_write: false };
            return Some(i as u64);
        }
    }
    None
}

/// Allocate a pipe pair.  On success writes `[read_fd, write_fd]` (i32) to
/// `pipefd_ptr` and returns 0.  `_flags` accepts O_CLOEXEC / O_NONBLOCK
/// (ignored — pipes are non-blocking).
pub(crate) fn sys_pipe2(pipefd_ptr: u64, _flags: u64) -> i64 {
    if pipefd_ptr == 0 { return -14; } // EFAULT
    // Reserve a pipe buffer slot.
    let pid = {
        let mut pipes = PIPES.lock();
        let mut found = usize::MAX;
        for (i, p) in pipes.iter_mut().enumerate() {
            if p.r_refs == 0 && p.w_refs == 0 {
                p.head = 0; p.tail = 0; p.len = 0;
                p.r_refs = 1; p.w_refs = 1;
                found = i;
                break;
            }
        }
        if found == usize::MAX { return -23; } // ENFILE
        found
    };
    // Allocate two FDs in the open-file table.
    let (read_fd, write_fd) = {
        let mut tbl = OPEN_FILES.lock();
        let mut rfd: i32 = -1;
        let mut wfd: i32 = -1;
        for (i, slot) in tbl.iter_mut().enumerate().skip(3) {
            if !slot.used {
                if rfd < 0 {
                    *slot = OpenFile { data: &[], offset: 0, used: true, is_dir: false, dir_path: [0; 192], dir_path_len: 0, pipe_id: pid as i16, pipe_is_write: false };
                    rfd = i as i32;
                } else {
                    *slot = OpenFile { data: &[], offset: 0, used: true, is_dir: false, dir_path: [0; 192], dir_path_len: 0, pipe_id: pid as i16, pipe_is_write: true };
                    wfd = i as i32;
                    break;
                }
            }
        }
        if rfd < 0 || wfd < 0 {
            // Roll back.
            if rfd >= 0 { tbl[rfd as usize].used = false; tbl[rfd as usize].pipe_id = -1; }
            let mut pipes = PIPES.lock();
            pipes[pid].r_refs = 0; pipes[pid].w_refs = 0;
            return -23;
        }
        (rfd, wfd)
    };
    unsafe {
        let arr = pipefd_ptr as *mut i32;
        core::ptr::write_unaligned(arr,           read_fd);
        core::ptr::write_unaligned(arr.add(1),    write_fd);
    }
    log::debug!("[pipe2] read_fd={} write_fd={} pipe_id={} flags={:#x}", read_fd, write_fd, pid, _flags);
    0
}

/// If `fd` is an open directory fd, return its stored path.
pub(crate) fn fd_dir_path(fd: u64) -> Option<alloc::string::String> {
    let idx = fd as usize;
    if idx >= MAX_OPEN_FILES { return None; }
    let tbl = OPEN_FILES.lock();
    if !tbl[idx].used || !tbl[idx].is_dir { return None; }
    let n = tbl[idx].dir_path_len;
    let s = core::str::from_utf8(&tbl[idx].dir_path[..n]).ok()?;
    Some(alloc::string::String::from(s))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Copy a byte slice from a user virtual address (minimal validation).
/// In a real kernel this would use access_ok + copy_from_user.
/// Here we accept the pointer directly as kernel == user space pre-M13.
pub(crate) unsafe fn read_user_bytes(ptr: u64, len: usize) -> Option<&'static [u8]> {
    if ptr == 0 || len > 0x200_0000 { return None; }
    Some(unsafe { core::slice::from_raw_parts(ptr as *const u8, len) })
}

pub(crate) unsafe fn write_user_bytes(ptr: u64, src: &[u8]) -> bool {
    if ptr == 0 || src.len() > 0x200_0000 {
        return false;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(src.as_ptr(), ptr as *mut u8, src.len());
    }
    true
}

// ── Syscall implementations ───────────────────────────────────────────────────

pub(crate) fn sys_write(fd: u64, buf_ptr: u64, len: u64) -> i64 {
    if fd == 5 || fd == 4 {
        log::warn!("[write-trace] pid={} fd={} len={}", crate::process::current_pid(), fd, len);
    }
    // Pipe write fast-path.
    let idx = fd as usize;

    // ── Synth-fd fast-path ────────────────────────────────────────────────────
    // Timerfd and eventfd handles are allocated by alloc_synth_fd() which starts
    // at SYNTH_FD_NEXT=64, i.e. exactly equal to MAX_OPEN_FILES.  The
    // `if idx >= 3 && idx < MAX_OPEN_FILES` block below therefore NEVER matches
    // synth fds, so writes to them fell through to the serial-print path without
    // updating the eventfd counter or calling epoll_wake.  That silently dropped
    // every Flutter engine task-post (write-1-to-eventfd), leaving the Dart UI
    // thread (pid=7) permanently blocked in epoll_wait with nothing to run.
    if idx >= MAX_OPEN_FILES {
        // Timerfd write
        {
            let mut tfd_tbl = TIMERFD_TABLE.lock();
            if let Some(ts) = tfd_tbl.get_mut(&(fd as u32)) {
                if len >= 8 && buf_ptr != 0 {
                    let count = unsafe { core::ptr::read_unaligned(buf_ptr as *const u64) };
                    ts.pending = ts.pending.saturating_add(count.max(1));
                } else {
                    ts.pending = ts.pending.saturating_add(1);
                }
                log::warn!("[sys_write] timerfd write fd={} pending={}", fd, ts.pending);
                drop(tfd_tbl);
                epoll_wake(fd as u32);
                return len as i64;
            }
        }
        // Eventfd write
        if EVENTFD_TABLE.lock().contains_key(&(fd as u32)) {
            if len >= 8 && buf_ptr != 0 {
                let val = unsafe { core::ptr::read_unaligned(buf_ptr as *const u64) };
                eventfd_write(fd as u32, val);
            } else {
                eventfd_write(fd as u32, 1);
            }
            return len as i64;
        }
        // Unknown synth fd — return success silently.
        return len as i64;
    }

    if idx >= 3 && idx < MAX_OPEN_FILES {
        let tbl = OPEN_FILES.lock();
        if tbl[idx].used && tbl[idx].pipe_id >= 0 {
            if !tbl[idx].pipe_is_write { return -9; } // EBADF (read end)
            let pid = tbl[idx].pipe_id as usize;
            drop(tbl);
            let src = match unsafe { read_user_bytes(buf_ptr, len as usize) } {
                Some(b) => b,
                None => return -14,
            };
            let mut pipes = PIPES.lock();
            let p = &mut pipes[pid];
            if p.r_refs == 0 { return -32; } // EPIPE
            let free = PIPE_BUF_SIZE - p.len;
            let n = src.len().min(free);
            for i in 0..n {
                p.buf[p.tail] = src[i];
                p.tail = (p.tail + 1) % PIPE_BUF_SIZE;
            }
            p.len += n;
            let written = n;
            {
                static PIPE_WRITE_LOG: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                // epfd=72 currently watches read_fd=4 on pipe_id=0; the paired
                // write end is fd=5. Trace this path to verify wake source.
                if fd == 5 || pid == 0 {
                    let k = PIPE_WRITE_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    if k < 32 {
                        log::warn!(
                            "[pipe-write] #{} pid={} fd={} pipe_id={} req_len={} wrote={} pipe_len={} free_before={}",
                            k,
                            crate::process::current_pid(),
                            fd,
                            pid,
                            len,
                            written,
                            p.len,
                            free
                        );
                    }
                }
            }
            drop(pipes);

            if written > 0 {
                let mut read_fds = alloc::vec::Vec::new();
                {
                    let open = OPEN_FILES.lock();
                    for r_fd in 0..MAX_OPEN_FILES {
                        if open[r_fd].used && open[r_fd].pipe_id == pid as i16 && !open[r_fd].pipe_is_write {
                            read_fds.push(r_fd as u32);
                        }
                    }
                }
                for r_fd in read_fds {
                    epoll_wake(r_fd);
                }
            }
            return if written == 0 { -11 } else { written as i64 }; // EAGAIN if full
        }
        drop(tbl);
        // Timerfd write: Dart uses write(tfd, &count, 8) to signal wakeup.
        // Treat as eventfd-style: add count to pending, so epoll delivers EPOLLIN.
        {
            let mut tfd_tbl = TIMERFD_TABLE.lock();
            if let Some(ts) = tfd_tbl.get_mut(&(fd as u32)) {
                if len >= 8 && buf_ptr != 0 {
                    let count = unsafe { core::ptr::read_unaligned(buf_ptr as *const u64) };
                    ts.pending = ts.pending.saturating_add(count.max(1));
                } else {
                    ts.pending = ts.pending.saturating_add(1);
                }
                log::warn!("[sys_write] timerfd write fd={} pending={}", fd, ts.pending);
                drop(tfd_tbl);
                epoll_wake(fd as u32);
                return len as i64;
            }
        }
        // Unknown synth fd write — log and return success to avoid breaking callers.
        if idx >= 64 {
            // Check if it's an eventfd before giving up.
            if EVENTFD_TABLE.lock().contains_key(&(fd as u32)) {
                if len >= 8 && buf_ptr != 0 {
                    let val = unsafe { core::ptr::read_unaligned(buf_ptr as *const u64) };
                    eventfd_write(fd as u32, val);
                } else {
                    eventfd_write(fd as u32, 1);
                }
                return len as i64;
            }
            log::warn!("[sys_write] unknown synth fd={} len={}", fd, len);
            return len as i64;
        }
    }
    let bytes = match unsafe { read_user_bytes(buf_ptr, len as usize) } {
        Some(b) => b,
        None => {
            log::warn!("[sys_write] EFAULT fd={} ptr={:#x} len={}", fd, buf_ptr, len);
            return -14; // EFAULT
        }
    };
    if let Ok(s) = core::str::from_utf8(bytes) {
        crate::logger::early_print(s);
    }
    len as i64
}

#[repr(C)]
struct LinuxIovec {
    base: u64,
    len: u64,
}

pub(crate) fn sys_writev(fd: u64, iov_ptr: u64, iov_cnt: u64) -> i64 {
    if iov_ptr == 0 || iov_cnt == 0 {
        return 0;
    }
    if iov_cnt > 1024 {
        return -22; // EINVAL
    }

    let mut total: i64 = 0;
    for i in 0..iov_cnt {
        let ent_ptr = iov_ptr + i * core::mem::size_of::<LinuxIovec>() as u64;
        let ent = unsafe { core::ptr::read_unaligned(ent_ptr as *const LinuxIovec) };
        if ent.len == 0 {
            continue;
        }
        let n = sys_write(fd, ent.base, ent.len);
        if n < 0 {
            return if total > 0 { total } else { n };
        }
        total += n;
        if n as u64 != ent.len {
            // Short write (e.g. pipe full): stop and return bytes written so far.
            break;
        }
    }
    total
}

pub(crate) fn sys_read(fd: u64, buf_ptr: u64, len: u64) -> i64 {
    if len == 0 { return 0; }
    if buf_ptr == 0 { return -14; } // EFAULT

    // Timerfd read: return expiry count (u64, 8 bytes) and drain the timer.
    // This is called by Flutter's TimerDrain() after epoll_wait signals EPOLLIN.
    {
        let mut tfd_tbl = TIMERFD_TABLE.lock();
        if let Some(ts) = tfd_tbl.get_mut(&(fd as u32)) {
            if len < 8 { return -22; } // EINVAL
            let now = monotonic_ns();
            // Accumulate any deadline-based expirations into pending.
            if ts.deadline_ns != 0 && now >= ts.deadline_ns {
                if ts.period_ns != 0 {
                    let elapsed = now.saturating_sub(ts.deadline_ns);
                    let n = 1u64 + elapsed / ts.period_ns;
                    ts.deadline_ns = ts.deadline_ns.saturating_add(n * ts.period_ns);
                    ts.pending = ts.pending.saturating_add(n);
                } else {
                    ts.deadline_ns = 0;
                    ts.pending = ts.pending.saturating_add(1);
                }
            }
            if ts.pending > 0 {
                let count = ts.pending;
                ts.pending = 0;
                drop(tfd_tbl);
                let pid = crate::process::current_pid();
                log::warn!("[tfd-read] pid={} tfd={} count={}", pid, fd, count);
                unsafe { core::ptr::write_unaligned(buf_ptr as *mut u64, count); }
                return 8;
            } else {
                // Not yet fired (or not armed) — EAGAIN.
                // Log for tfd=65 so we can trace why pending is 0 post-fix.
                if fd == 65 {
                    static TFD65_EAGAIN: core::sync::atomic::AtomicU32 =
                        core::sync::atomic::AtomicU32::new(0);
                    let k = TFD65_EAGAIN.fetch_add(1, Ordering::Relaxed);
                    if k < 64 {
                        let pid = crate::process::current_pid();
                        log::warn!("[tfd65-eagain] #{} pid={} deadline={}ns",
                            k, pid, ts.deadline_ns);
                    }
                }
                return -11;
            }
        }
    }

    // Eventfd read: drain counter atomically, return 8 bytes.
    {
        let mut efd_tbl = EVENTFD_TABLE.lock();
        if let Some(c) = efd_tbl.get_mut(&(fd as u32)) {
            if len < 8 { return -22; } // EINVAL
            if *c == 0 { return -11; } // EAGAIN (non-blocking)
            let val = *c;
            *c = 0;
            drop(efd_tbl);
            unsafe { core::ptr::write_unaligned(buf_ptr as *mut u64, val); }
            return 8;
        }
    }

    let to_read = len as usize;
    let mut tbl = OPEN_FILES.lock();
    let idx = fd as usize;
    if idx >= MAX_OPEN_FILES || !tbl[idx].used {
        return -9; // EBADF
    }
    // Pipe read fast-path.
    if tbl[idx].pipe_id >= 0 {
        if tbl[idx].pipe_is_write { return -9; } // EBADF (write end)
        let pid = tbl[idx].pipe_id as usize;
        drop(tbl);
        let mut pipes = PIPES.lock();
        let p = &mut pipes[pid];
        if p.len == 0 {
            // No data.  If all writers closed → EOF; otherwise EAGAIN.
            return if p.w_refs == 0 { 0 } else { -11 };
        }
        let n = to_read.min(p.len);
        let dst = buf_ptr as *mut u8;
        for i in 0..n {
            unsafe { *dst.add(i) = p.buf[p.head]; }
            p.head = (p.head + 1) % PIPE_BUF_SIZE;
        }
        p.len -= n;
        return n as i64;
    }
    let offset = tbl[idx].offset as usize;
    let data   = tbl[idx].data;
    if offset >= data.len() { return 0; } // EOF
    let available = data.len() - offset;
    let n = to_read.min(available);
    unsafe {
        core::ptr::copy_nonoverlapping(
            data.as_ptr().add(offset),
            buf_ptr as *mut u8,
            n,
        );
    }
    tbl[idx].offset += n as u64;
    n as i64
}

pub(crate) fn sys_open(path_ptr: u64, path_len: u64, _flags: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    let path = match core::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return -22, // EINVAL
    };
    // Strip null terminator if present.
    let path = path.trim_end_matches('\0');
    match crate::fs::lookup(path) {
        Some(data) => {
            match fd_alloc(data) {
                Some(fd) => {
                    log::debug!("[sys_open] '{}' → fd={} ({} bytes)", path, fd, data.len());
                    fd as i64
                }
                None => -23, // ENFILE (too many open files)
            }
        }
        None => {
            // File not found — maybe it's a directory.
            if crate::fs::is_dir(path) {
                match fd_alloc_dir(path) {
                    Some(fd) => {
                        log::debug!("[sys_open] '{}' → fd={} (dir)", path, fd);
                        return fd as i64;
                    }
                    None => return -23,
                }
            }
            log::debug!("[sys_open] '{}' → ENOENT", path);
            -2 // ENOENT
        }
    }
}

pub(crate) fn sys_close(fd: u64) -> i64 {
    let idx = fd as usize;
    if idx < 3 || idx >= MAX_OPEN_FILES { return 0; }
    let mut tbl = OPEN_FILES.lock();
    if tbl[idx].used {
        // If this is a pipe end, decrement the pipe's refcount.
        if tbl[idx].pipe_id >= 0 {
            let pid = tbl[idx].pipe_id as usize;
            let is_write = tbl[idx].pipe_is_write;
            tbl[idx].pipe_id = -1;
            drop(tbl);
            let mut pipes = PIPES.lock();
            if is_write && pipes[pid].w_refs > 0 { pipes[pid].w_refs -= 1; }
            if !is_write && pipes[pid].r_refs > 0 { pipes[pid].r_refs -= 1; }
            // Re-acquire to clear the file slot.
            drop(pipes);
            let mut tbl = OPEN_FILES.lock();
            tbl[idx].used = false;
            tbl[idx].data = &[];
            tbl[idx].offset = 0;
            return 0;
        }
        tbl[idx].used = false;
        tbl[idx].data = &[];
        tbl[idx].offset = 0;
    }
    0
}

pub(crate) fn sys_lseek(fd: u64, offset: i64, whence: u64) -> i64 {
    let idx = fd as usize;
    if idx < 3 || idx >= MAX_OPEN_FILES { return -9; } // EBADF
    let mut tbl = OPEN_FILES.lock();
    if !tbl[idx].used { return -9; }
    let size = tbl[idx].data.len() as i64;
    let new_off: i64 = match whence {
        0 => offset,                                    // SEEK_SET
        1 => tbl[idx].offset as i64 + offset,           // SEEK_CUR
        2 => size + offset,                             // SEEK_END
        _ => return -22, // EINVAL
    };
    if new_off < 0 { return -22; }
    tbl[idx].offset = new_off as u64;
    new_off
}

pub(crate) fn sys_fstat(fd: u64, stat_ptr: u64) -> i64 {
    // Minimal stat: only st_size matters for the Flutter engine.
    let idx = fd as usize;
    let (size, is_dir): (u64, bool) = if idx < 3 {
        (0, false)
    } else {
        let tbl = OPEN_FILES.lock();
        if !tbl[idx].used { return -9; }
        (tbl[idx].data.len() as u64, tbl[idx].is_dir)
    };
    if stat_ptr != 0 {
        // Write a minimal stat64 struct — st_size is at offset 48 in Linux stat64.
        unsafe {
            let p = stat_ptr as *mut u8;
            // Zero out 144 bytes (sizeof(struct stat64))
            core::ptr::write_bytes(p, 0, 144);
            // st_size at offset 48
            (p.add(48) as *mut u64).write_unaligned(size);
            // st_blksize at offset 56 = 4096
            (p.add(56) as *mut u64).write_unaligned(4096);
            // st_blocks at offset 64 = (size+511)/512
            (p.add(64) as *mut u64).write_unaligned((size + 511) / 512);
            // st_mode at offset 24: S_IFDIR|0755 for dirs, S_IFREG|0644 for files
            let mode: u32 = if is_dir { 0o040755 } else { 0o100644 };
            (p.add(24) as *mut u32).write_unaligned(mode);
        }
    }
    0
}

pub(crate) fn sys_getpid() -> i64 {
    let pid = crate::process::current_pid();
    if pid == 0 {
        return 1;
    }
    crate::process::get_group_leader(pid) as i64
}

pub(crate) fn sys_gettid() -> i64 {
    let pid = crate::process::current_pid();
    if pid == 0 { 1 } else { pid as i64 }
}

/// Linux `stat(const char *path, struct stat *buf)` — path is NUL-terminated.
/// Used by glibc `stat()`/`lstat()` and consequently by Flutter's
/// `fml::IsFile()` to verify `kernel_blob.bin` and similar assets exist.
pub(crate) fn sys_stat_path(path_ptr: u64, stat_ptr: u64) -> i64 {
    if path_ptr == 0 { return -14; } // EFAULT
    // Determine NUL-terminated length (bounded probe).
    let mut len: usize = 0;
    unsafe {
        let p = path_ptr as *const u8;
        while len < 4096 && *p.add(len) != 0 { len += 1; }
    }
    let fd = sys_open(path_ptr, len as u64, 0);
    if fd < 0 { return fd; }
    let r = sys_fstat(fd as u64, stat_ptr);
    let _ = sys_close(fd as u64);
    r
}

/// Linux `access(const char *path, int mode)` — returns 0 if file exists.
pub(crate) fn sys_access_path(path_ptr: u64, _mode: u64) -> i64 {
    if path_ptr == 0 { return -14; }
    let mut len: usize = 0;
    unsafe {
        let p = path_ptr as *const u8;
        while len < 4096 && *p.add(len) != 0 { len += 1; }
    }
    let fd = sys_open(path_ptr, len as u64, 0);
    if fd < 0 { return fd; }
    let _ = sys_close(fd as u64);
    0
}

/// Linux `newfstatat(int dirfd, const char *path, struct stat *buf, int flags)`.
/// We ignore `dirfd` (treat path as absolute) and delegate to `sys_stat_path`.
pub(crate) fn sys_newfstatat(_dirfd: u64, path_ptr: u64, stat_ptr: u64, _flags: u64) -> i64 {
    sys_stat_path(path_ptr, stat_ptr)
}

pub(crate) fn sys_exit(code: u64) -> i64 {
    let pid = crate::process::current_pid();
    if pid != 0 {
        // Read parent BEFORE exit() so we can wake them after.
        let parent_pid = crate::process::get_parent_of(pid);
        crate::process::exit(pid, code as i32);

        if let Some(ppid) = parent_pid {
            // We do NOT wake the parent here. Even if the parent is in the
            // `Blocked` state, that block could be for ANY reason —
            // futex_wait, pthread_cond_wait, etc — not just waitpid().
            // Clobbering the parent's RAX with our exit code makes
            // pthread_cond_wait return -6, which Flutter's libc++ wraps
            // and re-throws as "condition_variable wait failed" → abort.
            // The legitimate wake paths (cond_signal, futex_wake, real
            // waitpid polling) will reap the zombie themselves.
            log::warn!(
                "[trace] sys_exit: pid={} → parent ppid={} left untouched (zombie pending)",
                pid, ppid
            );
        }
        log::warn!("[trace] sys_exit: pid={} no parent — falling through to scheduler", pid);

        // On abnormal exit (negative code), wake all blocked threads so any
        // futex/cond_wait deadlock chain can unwind (e.g. vm-service death).
        if (code as i64) < 0 {
            let n_woken = {
                let mut count: u32 = 0;
                for wake_pid in 1..crate::process::MAX_PROCS as u32 {
                    if crate::process::is_blocked(wake_pid) {
                        crate::process::set_state(wake_pid, crate::process::ProcState::Running);
                        count += 1;
                    }
                }
                count
            };
            if n_woken > 0 {
                log::warn!("[trace] sys_exit: abnormal exit code={} — woke {} blocked threads",
                    code as i64, n_woken);
                FUTEX_WAITERS.lock().clear();
            }
        }

        if let Some(next_pid) = crate::process::next_runnable_pid(0) {
            crate::process::enter_user_by_pid_noreturn(next_pid);
        }
    }
    // No runnable user process left.
    loop {
        unsafe { core::arch::asm!("sti; hlt; cli", options(nomem, nostack)); }
    }
}

pub(crate) fn sys_waitpid(pid: u64, _status_ptr: u64, _options: u64) -> i64 {
    match crate::process::waitpid(pid as u32) {
        Ok(code) => code as i64,
        Err("not exited") => -11,      // EAGAIN
        Err("no such process") => -10, // ECHILD
        Err(_) => -22,                  // EINVAL
    }
}

/// Reap all zombie children of the calling process (PID 1 uses this for app hosts).
pub(crate) fn sys_reap_children() -> i64 {
    let parent = crate::process::current_pid();
    if parent == 0 {
        return -1;
    }
    crate::process::reap_zombie_children(parent) as i64
}

pub(crate) fn sys_kill(pid: u64, sig: u64) -> i64 {
    if sig != 9 {
        return -22; // EINVAL (only SIGKILL wired today)
    }
    match crate::process::kill(pid as u32) {
        Ok(()) => 0,
        Err(_) => -3, // ESRCH
    }
}

pub(crate) fn sys_arch_prctl(code: u64, addr: u64) -> i64 {
    // x86_64 Linux arch_prctl operations used by libc TLS setup.
    const ARCH_SET_FS: u64 = 0x1002;
    const ARCH_GET_FS: u64 = 0x1003;

    match code {
        ARCH_SET_FS => {
            crate::arch::cpu::set_fs_base(addr);
            let pid = crate::process::current_pid();
            if pid != 0 {
                crate::process::set_proc_fs_base(pid, addr);
            }
            0
        }
        ARCH_GET_FS => {
            if addr == 0 {
                return -14; // EFAULT
            }
            let fs = crate::arch::cpu::get_fs_base();
            unsafe { *(addr as *mut u64) = fs; }
            0
        }
        _ => -22, // EINVAL
    }
}

pub(crate) fn sys_exec(path_ptr: u64, path_len: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    let path = match core::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return -22,
    };
    let elf = match crate::fs::lookup(path) {
        Some(data) => data,
        None => return -2, // ENOENT
    };
    match crate::process::spawn(elf, path) {
        Ok(pid) => pid as i64,
        Err(_)  => -12, // ENOMEM
    }
}

/// Phase 47 — SYS_EXEC_WAIT (0x39E): spawn a child, block the calling
/// process, context-switch to the child immediately, and return the child's
/// exit code when it calls sys_exit.
///
/// This is a cooperative "exec-and-wait" that works without a preemptive
/// scheduler: the parent blocks, the child runs to completion, then sys_exit
/// restores the parent with RAX = exit_code.
pub(crate) fn sys_exec_wait(path_ptr: u64, path_len: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None    => return -14, // EFAULT
    };
    let path = match core::str::from_utf8(bytes) {
        Ok(s)  => s,
        Err(_) => return -22, // EINVAL
    };
    let elf = match crate::fs::lookup(path) {
        Some(data) => data,
        None       => return -2, // ENOENT
    };
    let parent_pid = crate::process::current_pid();
    if parent_pid == 0 { return -1; } // must have an active user process

    let child_pid = match crate::process::spawn(elf, path) {
        Ok(pid) => pid,
        Err(_)  => return -12, // ENOMEM
    };

    // Wire the child back to the parent so sys_exit knows who to wake.
    crate::process::set_child_parent(child_pid, parent_pid);

    // Save the parent's return context (RIP = next instr after syscall, RSP).
    let parent_rip = crate::arch::syscall::user_rip();
    let parent_rsp = crate::arch::syscall::user_rsp();
    crate::process::save_return_context(parent_pid, parent_rip, parent_rsp);
    crate::process::save_full_user_gprs(parent_pid);

    // Block the parent — it will be unblocked by the child's sys_exit.
    crate::process::set_state(parent_pid, crate::process::ProcState::Blocked);

    // Switch the kernel's notion of current PID and SYSRET into the child.
    crate::process::enter_user_by_pid_noreturn(child_pid)
}

pub(crate) fn sys_poweroff() -> i64 {
    log::info!("[syscall] sys_poweroff requested — triggering ACPI S5 shutdown");
    #[cfg(target_arch = "x86_64")]
    crate::arch::acpi_shutdown();
    #[cfg(not(target_arch = "x86_64"))]
    loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)); } }
}
