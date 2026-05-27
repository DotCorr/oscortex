//! Implementations of POSIX/glibc functions dispatched via syscall stubs
//! from the trampoline page mapped by posix_trampolines.rs.

use super::{read_user_bytes, write_user_bytes, monotonic_ns};
use crate::process::{self, dl::mmap_anon};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;

// ── Per-process TLS area ──────────────────────────────────────────────────
//
// Each process gets a 128 KiB TLS area starting at a bumped user VA.
// We store (pid → tls_base) here; the area is allocated on first use.

static TLS_TABLE: Mutex<BTreeMap<u32, u64>> = Mutex::new(BTreeMap::new());

// Per-thread emulated-TLS storage: (pid, obj_va) -> storage_va.
//
// CRITICAL: The "ptr" cell inside the __emutls_object (at obj+16) is shared
// across all threads because the object lives in libflutter_engine.so's
// .data. Caching the allocation there means thread A's storage becomes
// visible to thread B, which is the OPPOSITE of what TLS means. Threads
// then read each other's uninitialized slots (frequently NULL function
// pointers) and crash via call-thru-NULL. We must keep a per-thread cache
// here and NEVER write back into obj+16.
static EMUTLS_TABLE: Mutex<BTreeMap<(u32, u64), u64>> = Mutex::new(BTreeMap::new());

fn get_or_alloc_tls(pid: u32, pml4_phys: u64) -> u64 {
    let mut table = TLS_TABLE.lock();
    if let Some(&base) = table.get(&pid) {
        return base;
    }
    // Allocate 128 KiB for TLS in the current process.
    let pages = 32; // 32 × 4 KiB = 128 KiB
    let base = mmap_anon(pid, pml4_phys, 0, pages, 3);
    if base != u64::MAX {
        table.insert(pid, base);
    }
    base
}

// ── Per-process key-value store (pthread_key_*) ───────────────────────────

// key: (pid, thread_id, key_id) → value (u64)
static KEY_TABLE: Mutex<BTreeMap<(u32, u32, u32), u64>> = Mutex::new(BTreeMap::new());
static NEXT_KEY: AtomicU32 = AtomicU32::new(1);

// ── Per-thread pthread_self object ─────────────────────────────────────
//
// glibc/libstdc++ often treat pthread_t as a pointer to thread state and
// dereference fixed offsets (for example +0x68). Returning a small integer
// TID causes null-ish derefs inside C++ constructors.
//
// key: tid(pid) -> user VA of synthetic pthread object (256 bytes)
static PTHREAD_SELF_TABLE: Mutex<BTreeMap<u32, u64>> = Mutex::new(BTreeMap::new());

// ── Per-process malloc tracking ───────────────────────────────────────────
// (pid, user_va) → pages_allocated
// Used so free/realloc can skip already-allocated regions.
// Since munmap is a stub (no actual unmapping), free is a no-op here.

// ── Random state ─────────────────────────────────────────────────────────

static RAND_STATE: AtomicU64 = AtomicU64::new(1234567890);
static LOCALE_OBJ: AtomicU64 = AtomicU64::new(0);
static LOCALE_CURRENT: AtomicU64 = AtomicU64::new(0);

// ── Helpers ───────────────────────────────────────────────────────────────

fn pid_and_pml4() -> (u32, u64) {
    let pid = process::current_pid();
    let pml4 = process::get_user_context(pid)
        .map(|ctx| ctx.pml4_phys)
        .unwrap_or(0);
    (pid, pml4)
}

/// Read a null-terminated C string from user memory (max 4096 bytes).
unsafe fn read_cstr(ptr: u64) -> Option<&'static [u8]> {
    if ptr < 0x10000 || ptr >= 0x0000_8000_0000_0000 { return None; }
    let p = ptr as *const u8;
    let mut len = 0usize;
    unsafe {
        while len < 4096 && *p.add(len) != 0 { len += 1; }
    }
    Some(unsafe { core::slice::from_raw_parts(p, len) })
}

/// Write a value to a user pointer (8-byte atomic write).
unsafe fn write_u64_user(ptr: u64, v: u64) {
    if user_ptr_ok(ptr) { unsafe { *(ptr as *mut u64) = v; } }
}
unsafe fn write_u32_user(ptr: u64, v: u32) {
    if user_ptr_ok(ptr) { unsafe { *(ptr as *mut u32) = v; } }
}

// ── Memory allocation ─────────────────────────────────────────────────────

pub fn sys_malloc(size: u64) -> i64 {
    if size == 0 { return 8; } // non-null sentinel
    let (pid, pml4) = pid_and_pml4();
    if pid == 0 {
        log::error!("[malloc] pid=0 — no user context; size={:#x}", size);
        return 0;
    }
    // Progress counter — prints every 100K mallocs to show life signs
    static MALLOC_COUNT: core::sync::atomic::AtomicU64 =
        core::sync::atomic::AtomicU64::new(0);
    let n = MALLOC_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if n % 100_000 == 99_999 {
        log::warn!("[malloc] progress: {} mallocs pid={}", n + 1, pid);
    }
    // Allocate size + 16 bytes (header stores size for realloc).
    let alloc_size = size as usize + 16;
    let pages = alloc_size.div_ceil(4096);
    let va = mmap_anon(pid, pml4, 0, pages, 3);
    if va == u64::MAX {
        let used = crate::mm::frame_allocator::frames_used();
        let total = crate::mm::frame_allocator::frames_total();
        log::error!("[malloc] OOM: size={:#x} pages={} pid={} pml4={:#x} frames={}/{}", size, pages, pid, pml4, used, total);
        return 0;
    }
    // Write allocation size into header (kernel can write since user VA is
    // directly accessible in our single-space model).
    unsafe { write_u64_user(va, size); }
    (va + 16) as i64  // return ptr past header
}

pub fn sys_free(_ptr: u64) -> i64 {
    // munmap is a stub → leak is acceptable for demo.
    0
}

pub fn sys_calloc(n: u64, size: u64) -> i64 {
    // mmap_anon zeroes pages, so malloc already gives zeroed memory.
    sys_malloc(n.saturating_mul(size))
}

pub fn sys_realloc(ptr: u64, size: u64) -> i64 {
    if ptr == 0 { return sys_malloc(size); }
    if size == 0 { sys_free(ptr); return 0; }
    // Get old size from header (ptr - 16).
    let old_size = if ptr >= 16 {
        unsafe { *((ptr - 16) as *const u64) }
    } else { 0 };
    if old_size >= size { return ptr as i64; } // already big enough
    let new_ptr = sys_malloc(size);
    if new_ptr == 0 || ptr == 0 { return new_ptr; }
    // Copy old data.
    let copy_len = old_size.min(size) as usize;
    unsafe {
        core::ptr::copy_nonoverlapping(ptr as *const u8, new_ptr as *mut u8, copy_len);
    }
    new_ptr
}

pub fn sys_aligned_alloc(_align: u64, size: u64) -> i64 {
    // mmap always returns page-aligned memory.
    sys_malloc(size)
}

pub fn sys_posix_memalign(pptr: u64, _align: u64, size: u64) -> i64 {
    let ptr = sys_malloc(size);
    unsafe { write_u64_user(pptr, ptr as u64); }
    if ptr == 0 { 12 } else { 0 } // 12 = ENOMEM
}

pub fn sys_malloc_usable_size(ptr: u64) -> i64 {
    if ptr < 16 { return 0; }
    let size = unsafe { *((ptr - 16) as *const u64) };
    size as i64
}

pub fn sys_strdup(s: u64) -> i64 {
    let len = sys_strlen(s) as u64;
    let new_ptr = sys_malloc(len + 1);
    if new_ptr == 0 { return 0; }
    sys_memcpy(new_ptr as u64, s, len + 1);
    new_ptr
}

pub fn sys_strndup(s: u64, n: u64) -> i64 {
    let slen = sys_strlen(s) as u64;
    let len = slen.min(n);
    let new_ptr = sys_malloc(len + 1);
    if new_ptr == 0 { return 0; }
    sys_memcpy(new_ptr as u64, s, len);
    // null-terminate
    unsafe { *((new_ptr as u64 + len) as *mut u8) = 0; }
    new_ptr
}

// ── String operations ─────────────────────────────────────────────────────

pub fn sys_strlen(s: u64) -> i64 {
    if s < 0x10000 || s >= 0x0000_8000_0000_0000 { return 0; }  // guard: only userspace addresses
    let mut len = 0i64;
    unsafe {
        let mut p = s as *const u8;
        while *p != 0 { p = p.add(1); len += 1; if len > 1024*1024 { break; } }
    }
    len
}

#[inline(always)]
fn user_ptr_ok(p: u64) -> bool {
    p >= 0x1000 && p < 0x0000_8000_0000_0000
}

pub fn sys_memcpy(dst: u64, src: u64, n: u64) -> i64 {
    if n == 0 || !user_ptr_ok(dst) || !user_ptr_ok(src) { return dst as i64; }
    unsafe { core::ptr::copy_nonoverlapping(src as *const u8, dst as *mut u8, n as usize); }
    dst as i64
}

pub fn sys_memset(dst: u64, val: u64, n: u64) -> i64 {
    if n == 0 || !user_ptr_ok(dst) { return dst as i64; }
    unsafe { core::ptr::write_bytes(dst as *mut u8, val as u8, n as usize); }
    dst as i64
}

pub fn sys_memmove(dst: u64, src: u64, n: u64) -> i64 {
    if n == 0 || !user_ptr_ok(dst) || !user_ptr_ok(src) { return dst as i64; }
    unsafe { core::ptr::copy(src as *const u8, dst as *mut u8, n as usize); }
    dst as i64
}

pub fn sys_memcmp(a: u64, b: u64, n: u64) -> i64 {
    if n == 0 { return 0; }
    if !user_ptr_ok(a) || !user_ptr_ok(b) { return 0; }
    let sa = unsafe { core::slice::from_raw_parts(a as *const u8, n as usize) };
    let sb = unsafe { core::slice::from_raw_parts(b as *const u8, n as usize) };
    for i in 0..n as usize {
        let diff = sa[i] as i32 - sb[i] as i32;
        if diff != 0 { return diff as i64; }
    }
    0
}

pub fn sys_memchr(s: u64, c: u64, n: u64) -> i64 {
    if n == 0 || !user_ptr_ok(s) { return 0; }
    let slice = unsafe { core::slice::from_raw_parts(s as *const u8, n as usize) };
    for (i, &b) in slice.iter().enumerate() {
        if b == c as u8 { return (s + i as u64) as i64; }
    }
    0
}

pub fn sys_bzero(dst: u64, n: u64) -> i64 {
    sys_memset(dst, 0, n)
}

pub fn sys_strcmp(a: u64, b: u64) -> i64 {
    if a == 0 || b == 0 { return if a == b { 0 } else { 1 }; }
    let mut pa = a as *const u8;
    let mut pb = b as *const u8;
    loop {
        let ca = unsafe { *pa };
        let cb = unsafe { *pb };
        if ca != cb { return (ca as i64) - (cb as i64); }
        if ca == 0 { return 0; }
        pa = unsafe { pa.add(1) };
        pb = unsafe { pb.add(1) };
    }
}

pub fn sys_strncmp(a: u64, b: u64, n: u64) -> i64 {
    if n == 0 { return 0; }
    let mut pa = a as *const u8;
    let mut pb = b as *const u8;
    for _ in 0..n {
        let ca = unsafe { *pa };
        let cb = unsafe { *pb };
        if ca != cb { return (ca as i64) - (cb as i64); }
        if ca == 0 { return 0; }
        pa = unsafe { pa.add(1) };
        pb = unsafe { pb.add(1) };
    }
    0
}

pub fn sys_strcpy(dst: u64, src: u64) -> i64 {
    if dst == 0 || src == 0 { return dst as i64; }
    let mut ps = src as *const u8;
    let mut pd = dst as *mut u8;
    loop {
        let c = unsafe { *ps };
        unsafe { *pd = c; }
        if c == 0 { break; }
        ps = unsafe { ps.add(1) };
        pd = unsafe { pd.add(1) };
    }
    dst as i64
}

pub fn sys_strncpy(dst: u64, src: u64, n: u64) -> i64 {
    if dst == 0 { return 0; }
    let mut ps = src as *const u8;
    let mut pd = dst as *mut u8;
    for _ in 0..n {
        let c = if src != 0 { unsafe { let v = *ps; ps = ps.add(1); v } } else { 0 };
        unsafe { *pd = c; pd = pd.add(1); }
    }
    dst as i64
}

pub fn sys_strcat(dst: u64, src: u64) -> i64 {
    let dlen = sys_strlen(dst) as u64;
    sys_strcpy(dst + dlen, src);
    dst as i64
}

pub fn sys_strncat(dst: u64, src: u64, n: u64) -> i64 {
    let dlen = sys_strlen(dst) as u64;
    sys_strncpy(dst + dlen, src, n);
    // ensure null terminator
    let total = dlen + n;
    unsafe { *((dst + total) as *mut u8) = 0; }
    dst as i64
}

pub fn sys_strstr(hay: u64, needle: u64) -> i64 {
    if hay == 0 || needle == 0 { return 0; }
    let nlen = sys_strlen(needle) as usize;
    if nlen == 0 { return hay as i64; }
    let hlen = sys_strlen(hay) as usize;
    if nlen > hlen { return 0; }
    let hs = unsafe { core::slice::from_raw_parts(hay as *const u8, hlen) };
    let ns = unsafe { core::slice::from_raw_parts(needle as *const u8, nlen) };
    for i in 0..=(hlen - nlen) {
        if hs[i..i+nlen] == *ns { return (hay + i as u64) as i64; }
    }
    0
}

pub fn sys_strchr(s: u64, c: u64) -> i64 {
    if s == 0 { return 0; }
    let mut p = s as *const u8;
    loop {
        let b = unsafe { *p };
        if b == c as u8 { return p as i64; }
        if b == 0 { return 0; }
        p = unsafe { p.add(1) };
    }
}

pub fn sys_strrchr(s: u64, c: u64) -> i64 {
    if s == 0 { return 0; }
    let mut p = s as *const u8;
    let mut last = 0i64;
    loop {
        let b = unsafe { *p };
        if b == c as u8 { last = p as i64; }
        if b == 0 { break; }
        p = unsafe { p.add(1) };
    }
    last
}

pub fn sys_strnlen(s: u64, n: u64) -> i64 {
    if s == 0 { return 0; }
    let mut len = 0u64;
    let mut p = s as *const u8;
    while len < n {
        if unsafe { *p } == 0 { break; }
        p = unsafe { p.add(1) };
        len += 1;
    }
    len as i64
}

pub fn sys_strcspn(s: u64, reject: u64) -> i64 {
    if s == 0 { return 0; }
    let rlen = sys_strlen(reject) as usize;
    let rs = if reject != 0 { unsafe { core::slice::from_raw_parts(reject as *const u8, rlen) } } else { &[] };
    let mut len = 0i64;
    let mut p = s as *const u8;
    loop {
        let c = unsafe { *p };
        if c == 0 { break; }
        if rs.contains(&c) { break; }
        p = unsafe { p.add(1) };
        len += 1;
    }
    len
}

pub fn sys_strspn(s: u64, accept: u64) -> i64 {
    if s == 0 { return 0; }
    let alen = sys_strlen(accept) as usize;
    let acc = if accept != 0 { unsafe { core::slice::from_raw_parts(accept as *const u8, alen) } } else { &[] };
    let mut len = 0i64;
    let mut p = s as *const u8;
    loop {
        let c = unsafe { *p };
        if c == 0 || !acc.contains(&c) { break; }
        p = unsafe { p.add(1) };
        len += 1;
    }
    len
}

pub fn sys_strcasestr(hay: u64, needle: u64) -> i64 {
    // Simple case-insensitive substring search.
    if hay == 0 || needle == 0 { return 0; }
    let hlen = sys_strlen(hay) as usize;
    let nlen = sys_strlen(needle) as usize;
    if nlen == 0 { return hay as i64; }
    if nlen > hlen { return 0; }
    let hs = unsafe { core::slice::from_raw_parts(hay as *const u8, hlen) };
    let ns = unsafe { core::slice::from_raw_parts(needle as *const u8, nlen) };
    'outer: for i in 0..=(hlen - nlen) {
        for j in 0..nlen {
            if hs[i+j].to_ascii_lowercase() != ns[j].to_ascii_lowercase() {
                continue 'outer;
            }
        }
        return (hay + i as u64) as i64;
    }
    0
}

pub fn sys_strtol(s: u64, endptr: u64, base: u64) -> i64 {
    if s == 0 { return 0; }
    let bytes = unsafe { read_cstr(s) }.unwrap_or(&[]);
    let s = core::str::from_utf8(bytes).unwrap_or("").trim_start();
    let (neg, s) = if s.starts_with('-') { (true, &s[1..]) } else { (false, s) };
    let base = if base == 0 { 10 } else { base };
    let mut val: i64 = 0;
    let mut consumed = 0usize;
    for c in s.bytes() {
        let d = match c {
            b'0'..=b'9' => (c - b'0') as i64,
            b'a'..=b'f' => (c - b'a' + 10) as i64,
            b'A'..=b'F' => (c - b'A' + 10) as i64,
            _ => break,
        };
        if d >= base as i64 { break; }
        val = val * base as i64 + d;
        consumed += 1;
    }
    if endptr != 0 {
        unsafe { *(endptr as *mut u64) = s.as_ptr() as u64 + consumed as u64 + if neg { 1 } else { 0 }; }
    }
    if neg { -val } else { val }
}

pub fn sys_strtoul(s: u64, endptr: u64, base: u64) -> i64 {
    sys_strtol(s, endptr, base)
}

pub fn sys_strtoll(s: u64, endptr: u64, base: u64) -> i64 {
    sys_strtol(s, endptr, base)
}

pub fn sys_strtoull(s: u64, endptr: u64, base: u64) -> i64 {
    sys_strtol(s, endptr, base)
}

pub fn sys_atoi(s: u64) -> i64 {
    sys_strtol(s, 0, 10)
}

pub fn sys_rand() -> i64 {
    // Xorshift64
    let mut x = RAND_STATE.load(Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    RAND_STATE.store(x, Ordering::Relaxed);
    (x & 0x7FFFFFFF) as i64
}

pub fn sys_srand(seed: u32) {
    RAND_STATE.store(seed as u64 | 1, Ordering::Relaxed);
}

// ── Threading ─────────────────────────────────────────────────────────────

pub fn sys_pthread_self() -> i64 {
    let tid = process::current_pid();
    if tid == 0 { return 0; }

    let fs_base = crate::process::get_fs_base(tid);
    if fs_base != 0 {
        return fs_base as i64;
    }

    // Fast path: already allocated for this tid.
    if let Some(&va) = PTHREAD_SELF_TABLE.lock().get(&tid) {
        return va as i64;
    }

    // Allocate a synthetic thread object in user VA and publish it.
    // Keep it larger than common pthread header offsets used by runtimes.
    let obj = sys_malloc(256) as u64;
    if obj == 0 {
        // OOM fallback: return a non-null tagged value rather than 0.
        return ((tid as u64) << 12 | 1) as i64;
    }

    unsafe {
        // Clear object and set a few self-referential fields expected by
        // pointer-based pthread_t implementations.
        core::ptr::write_bytes(obj as *mut u8, 0, 256);
        *(obj as *mut u64) = obj;              // self pointer
        *((obj + 8) as *mut u64) = tid as u64; // tid
        *((obj + 0x68) as *mut u64) = obj;     // non-null guard field
    }

    PTHREAD_SELF_TABLE.lock().insert(tid, obj);
    obj as i64
}

pub fn sys_pthread_mutex_init(mutex: u64) -> i64 {
    // A mutex is a 64-bit value: 0 = unlocked.
    if mutex != 0 { unsafe { *(mutex as *mut u64) = 0; } }
    0
}

pub fn sys_pthread_mutex_lock(mutex: u64, sys_nr: u64) -> i64 {
    if mutex < 0x1000 {
        log::warn!("[mutex] pthread_mutex_lock bogus addr={:#x}, returning 0", mutex);
        return 0;
    }
    let atom = unsafe { &*(mutex as *const core::sync::atomic::AtomicU32) };
    let pid = crate::process::current_pid();
    // Targeted trace for the mutex that pid=3 holds across epoll_wait, blocking pid=2/8/9.
    if mutex == 0x394000018 {
        static MUTEX394_LOCK_LOG: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let ml = MUTEX394_LOCK_LOG.fetch_add(1, Ordering::Relaxed);
        if ml < 40 {
            let rip = crate::arch::syscall::user_rip();
            let val = atom.load(Ordering::Relaxed);
            log::warn!("[mutex394-lock] #{} pid={} rip={:#x} current_val={}", ml, pid, rip, val);
        }
    }

    loop {
        let res = atom.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire);
        if res.is_ok() {
            return 0;
        }

        if pid == 0 {
            return 0;
        }

        // Add the process to the futex waiters list
        super::futex_waiter_add(mutex, pid);

        // Yield if there is a sibling runnable thread
        if let Some(next) = crate::process::next_runnable_pid(pid) {
            if next != pid {
                let urip = crate::arch::syscall::user_rip();
                let ursp = crate::arch::syscall::user_rsp();
                // Save context pointing to the syscall instruction itself so we retry on re-entry.
                crate::process::save_return_context(pid, urip - 2, ursp);
                crate::process::save_full_user_gprs(pid);
                crate::process::set_rax(pid, sys_nr); // Restore the original syscall number
                crate::process::save_xstate(pid);
                crate::process::set_state(pid, crate::process::ProcState::Blocked);
                crate::process::enter_user_by_pid_noreturn(next);
            }
        }

        // No sibling thread — loop in kernel space using hlt
        while atom.load(Ordering::Acquire) != 0 && super::futex_waiter_present(mutex, pid) {
            #[cfg(target_arch = "x86_64")]
            unsafe {
                core::arch::asm!("sti; hlt; cli", options(nomem, nostack));
            }
            if let Some(next) = crate::process::next_runnable_pid(pid) {
                if next != pid {
                    let urip = crate::arch::syscall::user_rip();
                    let ursp = crate::arch::syscall::user_rsp();
                    crate::process::save_return_context(pid, urip - 2, ursp);
                    crate::process::save_full_user_gprs(pid);
                    crate::process::set_rax(pid, sys_nr);
                    crate::process::save_xstate(pid);
                    crate::process::set_state(pid, crate::process::ProcState::Blocked);
                    crate::process::enter_user_by_pid_noreturn(next);
                }
            }
        }

        super::futex_waiter_remove(mutex, pid);
    }
}

pub fn sys_pthread_mutex_unlock(mutex: u64) -> i64 {
    if mutex < 0x1000 { return 0; }
    let atom = unsafe { &*(mutex as *const core::sync::atomic::AtomicU32) };
    atom.store(0, Ordering::Release);
    // Targeted trace for the mutex pid=3 holds across epoll_wait.
    if mutex == 0x394000018 {
        static MUTEX394_UNLOCK_LOG: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let ul = MUTEX394_UNLOCK_LOG.fetch_add(1, Ordering::Relaxed);
        if ul < 40 {
            let pid = crate::process::current_pid();
            let rip = crate::arch::syscall::user_rip();
            log::warn!("[mutex394-unlock] #{} pid={} rip={:#x}", ul, pid, rip);
        }
    }
    let n = super::futex_wake_waiters(mutex, 1);
    // Deferred task-runner kick: consume the KICK_REQUESTED flag set by the
    // APIC ISR and call force_wake_all_task_runners here in syscall context
    // (where spinlock acquisition is safe, unlike the ISR).  This fires at
    // most every ~500 ms (30 APIC ticks) and wakes task runners stuck in
    // infinite epoll_wait even when sys_wm_event_wait is not being called
    // (e.g. while the embedder is inside run_task_fn).
    if super::KICK_REQUESTED.swap(false, Ordering::AcqRel) {
        let _ = super::force_wake_all_task_runners("deferred-kick");
    }
    let wpid = crate::process::current_pid();
    if wpid != 0 && n > 0 {
        if let Some(next) = crate::process::next_runnable_pid(wpid) {
            if next != wpid {
                let urip = crate::arch::syscall::user_rip();
                let ursp = crate::arch::syscall::user_rsp();
                crate::process::save_return_context(wpid, urip, ursp);
                crate::process::save_full_user_gprs(wpid);
                crate::process::set_rax(wpid, 0);
                crate::process::save_xstate(wpid);
                crate::process::enter_user_by_pid_noreturn(next);
            }
        }
    }
    0
}

pub fn sys_pthread_mutex_trylock(mutex: u64) -> i64 {
    if mutex < 0x1000 { return 0; }
    let atom = unsafe { &*(mutex as *const core::sync::atomic::AtomicU32) };
    if atom.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_ok() { 0 } else { 16 }
}

pub fn sys_pthread_once(once: u64, func: u64, sys_nr: u64) -> i64 {
    if once < 0x1000 || func == 0 {
        log::error!("[pthread-err] once EINVAL pid={} once={:#x} func={:#x} rip={:#x}",
            crate::process::current_pid(), once, func, crate::arch::syscall::user_rip());
        return 22;
    }
    let atom = unsafe { &*(once as *const core::sync::atomic::AtomicU32) };
    let pid = crate::process::current_pid();

    loop {
        // State: 0=uninit, 1=in-progress, 2=done
        let state = atom.load(Ordering::Acquire);
        if state == 2 {
            return 0;
        }
        if state == 0 {
            if atom.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_ok() {
                // We won the race — call func.
                let f: extern "C" fn() = unsafe { core::mem::transmute(func) };
                f();
                atom.store(2, Ordering::Release);
                // Wake all threads blocked waiting for the init to complete.
                let n = super::futex_wake_waiters(once, u32::MAX);
                let wpid = crate::process::current_pid();
                if wpid != 0 && n > 0 {
                    if let Some(next) = crate::process::next_runnable_pid(wpid) {
                        if next != wpid {
                            let urip = crate::arch::syscall::user_rip();
                            let ursp = crate::arch::syscall::user_rsp();
                            crate::process::save_return_context(wpid, urip, ursp);
                            crate::process::save_full_user_gprs(wpid);
                            crate::process::set_rax(wpid, 0);
                            crate::process::save_xstate(wpid);
                            crate::process::enter_user_by_pid_noreturn(next);
                        }
                    }
                }
                return 0;
            }
        }

        // If state == 1, block until it becomes 2.
        if pid == 0 {
            return 0;
        }

        super::futex_waiter_add(once, pid);

        if let Some(next) = crate::process::next_runnable_pid(pid) {
            if next != pid {
                let urip = crate::arch::syscall::user_rip();
                let ursp = crate::arch::syscall::user_rsp();
                // Save context pointing to the syscall instruction itself so we retry on re-entry.
                crate::process::save_return_context(pid, urip - 2, ursp);
                crate::process::save_full_user_gprs(pid);
                crate::process::set_rax(pid, sys_nr); // sys_pthread_once syscall number
                crate::process::save_xstate(pid);
                crate::process::set_state(pid, crate::process::ProcState::Blocked);
                crate::process::enter_user_by_pid_noreturn(next);
            }
        }

        // No sibling runnable thread — loop using hlt in the kernel until state is 2.
        while atom.load(Ordering::Acquire) != 2 && super::futex_waiter_present(once, pid) {
            #[cfg(target_arch = "x86_64")]
            unsafe {
                core::arch::asm!("sti; hlt; cli", options(nomem, nostack));
            }
            if let Some(next) = crate::process::next_runnable_pid(pid) {
                if next != pid {
                    let urip = crate::arch::syscall::user_rip();
                    let ursp = crate::arch::syscall::user_rsp();
                    crate::process::save_return_context(pid, urip - 2, ursp);
                    crate::process::save_full_user_gprs(pid);
                    crate::process::set_rax(pid, sys_nr);
                    crate::process::save_xstate(pid);
                    crate::process::set_state(pid, crate::process::ProcState::Blocked);
                    crate::process::enter_user_by_pid_noreturn(next);
                }
            }
        }

        super::futex_waiter_remove(once, pid);
    }
}

pub fn sys_pthread_key_create(key_ptr: u64, _dtor: u64) -> i64 {
    let key = NEXT_KEY.fetch_add(1, Ordering::Relaxed);
    unsafe { write_u32_user(key_ptr, key); }
    0
}

pub fn sys_pthread_setspecific(key: u64, value: u64) -> i64 {
    let pid = process::current_pid();
    let tid = pid; // simplified: single thread per process
    KEY_TABLE.lock().insert((pid, tid, key as u32), value);
    0
}

pub fn sys_pthread_getspecific(key: u64) -> i64 {
    let pid = process::current_pid();
    let tid = pid;
    KEY_TABLE.lock().get(&(pid, tid, key as u32)).copied().unwrap_or(0) as i64
}

pub fn sys_pthread_cond_wait_timeout(cond: u64, mutex: u64, timeout_ns: u64, sys_nr: u64) -> i64 {
    if cond < 0x1000 || mutex < 0x1000 {
        log::error!("[pthread-err] cond_wait EINVAL pid={} cond={:#x} mutex={:#x} rip={:#x}",
            crate::process::current_pid(), cond, mutex, crate::arch::syscall::user_rip());
        return 22;
    }

    let pid = crate::process::current_pid();
    let atom = unsafe { &*(cond as *const core::sync::atomic::AtomicU32) };

    // Check if we have an active wait state
    let mut state = {
        let table = super::COND_WAIT_STATE.lock();
        table.get(&pid).copied()
    };

    if state.is_none() {
        // First time: register waiter, then unlock mutex and enter Waiting state.
        let seq = atom.load(Ordering::Acquire);
        if pid != 0 {
            super::futex_waiter_add(cond, pid);
        }
        sys_pthread_mutex_unlock(mutex);
        let next_state = super::CondWaitState::Waiting { cond, mutex, seq, timeout_ns };
        super::COND_WAIT_STATE.lock().insert(pid, next_state);
        state = Some(next_state);
    }

    // Now process the state machine
    loop {
        match state.unwrap() {
            super::CondWaitState::Waiting { cond, mutex, seq, timeout_ns } => {
                let cur_seq = atom.load(Ordering::Acquire);
                if cur_seq != seq {
                    // The condvar was signaled!
                    super::futex_waiter_remove(cond, pid);
                    let next_state = super::CondWaitState::AcquiringMutex { mutex, timed_out: false };
                    super::COND_WAIT_STATE.lock().insert(pid, next_state);
                    state = Some(next_state);
                    // Continue to try and acquire the mutex
                    continue;
                }

                // Check for timeout
                if timeout_ns != 0 && monotonic_ns() >= timeout_ns {
                    super::futex_waiter_remove(cond, pid);
                    let next_state = super::CondWaitState::AcquiringMutex { mutex, timed_out: true };
                    super::COND_WAIT_STATE.lock().insert(pid, next_state);
                    state = Some(next_state);
                    continue;
                }

                // Otherwise, we must wait (yield or hlt)
                if pid != 0 {
                    super::futex_waiter_add(cond, pid);
                    if let Some(next) = crate::process::next_runnable_pid(pid) {
                        if next != pid {
                            let urip = crate::arch::syscall::user_rip();
                            let ursp = crate::arch::syscall::user_rsp();
                            crate::process::save_return_context(pid, urip - 2, ursp);
                            crate::process::save_full_user_gprs(pid);
                            crate::process::set_rax(pid, sys_nr); // Restore the original syscall number
                            crate::process::save_xstate(pid);
                            crate::process::set_state(pid, crate::process::ProcState::Blocked);
                            crate::process::enter_user_by_pid_noreturn(next);
                        }
                    }
                }

                // No sibling runnable thread — loop using hlt in the kernel until sequence changes or timeout
                while atom.load(Ordering::Acquire) == seq 
                    && (timeout_ns == 0 || monotonic_ns() < timeout_ns)
                    && super::futex_waiter_present(cond, pid) 
                {
                    #[cfg(target_arch = "x86_64")]
                    unsafe {
                        core::arch::asm!("sti; hlt; cli", options(nomem, nostack));
                    }
                    if let Some(next) = crate::process::next_runnable_pid(pid) {
                        if next != pid {
                            let urip = crate::arch::syscall::user_rip();
                            let ursp = crate::arch::syscall::user_rsp();
                            crate::process::save_return_context(pid, urip - 2, ursp);
                            crate::process::save_full_user_gprs(pid);
                            crate::process::set_rax(pid, sys_nr);
                            crate::process::save_xstate(pid);
                            crate::process::set_state(pid, crate::process::ProcState::Blocked);
                            crate::process::enter_user_by_pid_noreturn(next);
                        }
                    }
                }

                // Re-evaluate condition after waking up
                let cur_seq = atom.load(Ordering::Acquire);
                if cur_seq != seq {
                    super::futex_waiter_remove(cond, pid);
                    let next_state = super::CondWaitState::AcquiringMutex { mutex, timed_out: false };
                    super::COND_WAIT_STATE.lock().insert(pid, next_state);
                    state = Some(next_state);
                    continue;
                }

                if timeout_ns != 0 && monotonic_ns() >= timeout_ns {
                    super::futex_waiter_remove(cond, pid);
                    let next_state = super::CondWaitState::AcquiringMutex { mutex, timed_out: true };
                    super::COND_WAIT_STATE.lock().insert(pid, next_state);
                    state = Some(next_state);
                    continue;
                }
            }

            super::CondWaitState::AcquiringMutex { mutex, timed_out } => {
                // Try to acquire the mutex (using the new sys_pthread_mutex_lock logic, retrying sys_nr on yield)
                let rc = sys_pthread_mutex_lock(mutex, sys_nr);
                if rc == 0 {
                    // Successfully acquired the mutex! Clear wait state and return
                    super::COND_WAIT_STATE.lock().remove(&pid);
                    return if timed_out { 110 } else { 0 }; // 110 = ETIMEDOUT
                } else {
                    // yielded inside sys_pthread_mutex_lock
                    return rc;
                }
            }
        }
    }
}

pub fn sys_pthread_cond_wait(cond: u64, mutex: u64, sys_nr: u64) -> i64 {
    sys_pthread_cond_wait_timeout(cond, mutex, 0, sys_nr)
}

pub fn sys_pthread_cond_timedwait(cond: u64, mutex: u64, timeout: u64, sys_nr: u64) -> i64 {
    let (sec, nsec) = if timeout != 0 {
        unsafe {
            (
                core::ptr::read_unaligned(timeout as *const i64),
                core::ptr::read_unaligned((timeout + 8) as *const i64),
            )
        }
    } else {
        (0, 0)
    };
    let timeout_ns = if timeout != 0 {
        (sec.max(0) as u64).saturating_mul(1_000_000_000)
            .saturating_add(nsec.max(0) as u64)
    } else {
        0
    };

    static TIMEDWAIT_LOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let n = TIMEDWAIT_LOG.fetch_add(1, Ordering::Relaxed);
    if n < 32 {
        log::warn!("[timedwait] #{} pid={} cond={:#x} mutex={:#x} timeout_ns={} now={}",
            n, crate::process::current_pid(), cond, mutex, timeout_ns, monotonic_ns());
    }

    sys_pthread_cond_wait_timeout(cond, mutex, timeout_ns, sys_nr)
}

#[inline]
fn cond_broadcast_loop_handoff(_pid: u32, _cond: u64, _n: i64, _bridged: u32) {}

pub fn sys_pthread_cond_signal(cond: u64) -> i64 {
    if cond == 0 { return 22; }
    let pid = crate::process::current_pid();
    let atom = unsafe { &*(cond as *const core::sync::atomic::AtomicU32) };
    let old_seq = atom.fetch_add(1, Ordering::Release);
    let n = super::futex_wake_waiters(cond, 1);
    let mut bridged = 0u32;
    if n == 0 {
        bridged = super::cond_miss_bridge(cond, 1);
        if bridged > 0 {
            static COND_SIGNAL_BRIDGED_LOG: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let k = COND_SIGNAL_BRIDGED_LOG.fetch_add(1, Ordering::Relaxed);
            if k < 16 || k % 256 == 0 {
                log::warn!(
                    "[cond-signal-bridged] #{} pid={} cond={:#x} woke={} bridged={}",
                    k,
                    pid,
                    cond,
                    n,
                    bridged
                );
            }
        }
    }
    static COND_SIGNAL_LOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let k = COND_SIGNAL_LOG.fetch_add(1, Ordering::Relaxed);
    if k < 64 || k % 256 == 0 {
        log::warn!("[cond-signal] #{} pid={} cond={:#x} woke={}", k, pid, cond, n);
    }
    log::trace!("[cond-signal] pid={} cond={:#x} seq {}→{} woke={}", pid, cond, old_seq, old_seq+1, n);
    let wpid = crate::process::current_pid();
    if wpid != 0 && (n > 0 || bridged > 0) {
        if let Some(next) = crate::process::next_runnable_pid(wpid) {
            if next != wpid {
                let urip = crate::arch::syscall::user_rip();
                let ursp = crate::arch::syscall::user_rsp();
                crate::process::save_return_context(wpid, urip, ursp);
                crate::process::save_full_user_gprs(wpid);
                crate::process::set_rax(wpid, 0);
                crate::process::save_xstate(wpid);
                crate::process::enter_user_by_pid_noreturn(next);
            }
        }
    }
    0
}

pub fn sys_pthread_cond_broadcast(cond: u64) -> i64 {
    if cond == 0 { return 22; }
    let pid = crate::process::current_pid();
    let atom = unsafe { &*(cond as *const core::sync::atomic::AtomicU32) };
    let old_seq = atom.fetch_add(1, Ordering::Release);
    let n = super::futex_wake_waiters(cond, i32::MAX as u32);
    let mut bridged = 0u32;
    let mut skip_bridge = false;
    if n == 0 && pid == 2 {
        let ursp = crate::arch::syscall::user_rsp();
        let cur_cr3 = crate::arch::memory::read_cr3() & 0x000f_ffff_ffff_f000;
        if ursp != 0 && crate::mm::paging::translate_user_page(cur_cr3, ursp & !0xfff).is_some() {
            let caller = unsafe { core::ptr::read_unaligned(ursp as *const u64) };
            // Flutter engine Dart VM `ConditionVariable::NotifyAll` callsite
            // after `pthread_cond_broadcast@plt` (see mapped addr 0x326d61e).
            // Bridging this path causes a synthetic wake storm.
            if caller == 0x326d61e {
                skip_bridge = true;
            }
        }
    }

    // Diagnostic only — log zero-wake cond broadcasts from the engine worker thread.
    if pid == 2 && n == 0 {
        static COND_BROADCAST_ZERO_WAKE_LOG: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let z = COND_BROADCAST_ZERO_WAKE_LOG.fetch_add(1, Ordering::Relaxed);
        if z < 16 || z % 4096 == 0 {
            log::warn!(
                "[cond-broadcast-zero-wake] #{} pid={} cond={:#x} skip_bridge={}",
                z,
                pid,
                cond,
                skip_bridge
            );
        }
    }

    if n == 0 && !skip_bridge {
        bridged = super::cond_miss_bridge(cond, i32::MAX as u32);
        if bridged > 0 {
            static COND_BROADCAST_BRIDGED_LOG: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let k = COND_BROADCAST_BRIDGED_LOG.fetch_add(1, Ordering::Relaxed);
            if k < 16 || k % 512 == 0 {
                log::warn!(
                    "[cond-broadcast-bridged] #{} pid={} cond={:#x} woke={} bridged={}",
                    k,
                    pid,
                    cond,
                    n,
                    bridged
                );
            }
        }
    }
    cond_broadcast_loop_handoff(pid, cond, n, bridged);

    static COND_BROADCAST_LOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let k = COND_BROADCAST_LOG.fetch_add(1, Ordering::Relaxed);
    if k < 64 || k % 2048 == 0 {
        log::warn!("[cond-broadcast] #{} pid={} cond={:#x} woke={}", k, pid, cond, n);
    }
    log::trace!(
        "[cond-broadcast] pid={} cond={:#x} seq {}→{} woke={}",
        pid,
        cond,
        old_seq,
        old_seq + 1,
        n
    );
    
    let wpid = crate::process::current_pid();
    if wpid != 0 && (n > 0 || bridged > 0) {
        if let Some(next) = crate::process::next_runnable_pid(wpid) {
            if next != wpid {
                let urip = crate::arch::syscall::user_rip();
                let ursp = crate::arch::syscall::user_rsp();
                crate::process::save_return_context(wpid, urip, ursp);
                crate::process::save_full_user_gprs(wpid);
                crate::process::set_rax(wpid, 0);
                crate::process::save_xstate(wpid);
                crate::process::enter_user_by_pid_noreturn(next);
            }
        }
    }
    0
}

pub fn sys_pthread_attr_init(attr: u64) -> i64 {
    log::trace!("[trace] sys_pthread_attr_init attr={:#x}", attr);
    // pthread_attr_t is typically 56 bytes; zero it out.
    if attr != 0 { unsafe { core::ptr::write_bytes(attr as *mut u8, 0, 56); } }
    0
}

pub fn sys_pthread_attr_destroy(attr: u64) -> i64 {
    log::trace!("[trace] sys_pthread_attr_destroy attr={:#x}", attr);
    0
}

pub fn sys_pthread_attr_setstacksize(attr: u64, stacksize: u64) -> i64 {
    log::trace!("[trace] sys_pthread_attr_setstacksize attr={:#x} size={:#x}", attr, stacksize);
    // Store stacksize at offset 8 of the attr struct (glibc layout).
    if attr != 0 { unsafe { *((attr + 8) as *mut u64) = stacksize; } }
    0
}

pub fn sys_pthread_attr_setdetachstate(attr: u64, state: u64) -> i64 {
    log::trace!("[trace] sys_pthread_attr_setdetachstate attr={:#x} state={}", attr, state);
    // Store detach state at offset 0.
    if attr != 0 { unsafe { *(attr as *mut u64) = state; } }
    0
}

pub fn sys_pthread_attr_getstack(attr: u64, base_out: u64, size_out: u64) -> i64 {
    // The Dart VM calls pthread_getattr_np(self, &attr) then
    // pthread_attr_getstack(&attr, &base, &size) to validate stack bounds.
    // If we return 0,0 the VM aborts with "GetAndValidateThreadStackBounds failed".
    // Look up the current thread's recorded user-space stack bounds.
    let tid = crate::process::current_pid();
    let (base, size) = crate::process::get_user_stack_bounds(tid);

    // Also check if the attr struct was pre-filled by pthread_getattr_np
    // (stored at attr+0x10 = base, attr+0x18 = size).
    let (eff_base, eff_size) = if base != 0 {
        (base, size)
    } else if attr >= 0x1000 {
        // Try reading what pthread_getattr_np may have written into attr.
        let a_base = unsafe { *(attr.wrapping_add(0x10) as *const u64) };
        let a_size = unsafe { *(attr.wrapping_add(0x18) as *const u64) };
        if a_base != 0 && a_size != 0 { (a_base, a_size) } else { (0, 0) }
    } else {
        (0, 0)
    };

    log::debug!("[pthread_attr_getstack] tid={} base={:#x} size={:#x}", tid, eff_base, eff_size);
    unsafe {
        if base_out != 0 { *(base_out as *mut u64) = eff_base; }
        if size_out != 0 { *(size_out as *mut u64) = eff_size; }
    }
    0
}

pub fn sys_pthread_setname_np(thread: u64, name: u64) -> i64 {
    let tid = if thread > 0x100000 {
        crate::process::find_tid_by_fs_base(thread).unwrap_or(0)
    } else {
        thread as u32
    };
    let name_str = if name > 0x1000 {
        let bytes = unsafe {
            let p = name as *const u8;
            let mut end = 0usize;
            while end < 64 && *p.add(end) != 0 { end += 1; }
            core::slice::from_raw_parts(p, end)
        };
        core::str::from_utf8(bytes).unwrap_or("?")
    } else { "?" };
    log::trace!("[trace] sys_pthread_setname_np thread={:#x} (tid={}) name=\"{}\"", thread, tid, name_str);
    0
}

pub fn sys_pthread_attr_getter_noop(nr: u64, a0: u64, a1: u64, a2: u64) -> i64 {
    log::trace!("[trace] pthread_attr/sched noop nr={:#x} a0={:#x} a1={:#x} a2={:#x}", nr, a0, a1, a2);
    0
}

// ── Semaphores ────────────────────────────────────────────────────────────

pub fn sys_sem_init(sem: u64, _pshared: u64, value: u64) -> i64 {
    if sem == 0 {
        log::error!("[pthread-err] sem_init EINVAL pid={} rip={:#x}",
            crate::process::current_pid(), crate::arch::syscall::user_rip());
        return 22;
    }
    unsafe { *(sem as *mut u32) = value as u32; }
    0
}

pub fn sys_sem_wait(sem: u64) -> i64 {
    if sem < 0x1000 {
        log::error!("[pthread-err] sem_wait EINVAL pid={} sem={:#x} rip={:#x}",
            crate::process::current_pid(), sem, crate::arch::syscall::user_rip());
        return 22;
    }
    let atom = unsafe { &*(sem as *const core::sync::atomic::AtomicU32) };
    loop {
        let v = atom.load(Ordering::Acquire);
        if v > 0 && atom.compare_exchange(v, v - 1, Ordering::Acquire, Ordering::Acquire).is_ok() {
            break;
        }
        unsafe { core::arch::asm!("pause", options(nomem, nostack, preserves_flags)); }
    }
    0
}

pub fn sys_sem_trywait(sem: u64) -> i64 {
    if sem < 0x1000 { return -11; }
    let atom = unsafe { &*(sem as *const core::sync::atomic::AtomicU32) };
    let v = atom.load(Ordering::Acquire);
    if v == 0 { return -11; }
    if atom.compare_exchange(v, v - 1, Ordering::Acquire, Ordering::Acquire).is_ok() { 0 } else { -11 }
}

pub fn sys_sem_post(sem: u64) -> i64 {
    if sem == 0 {
        log::error!("[pthread-err] sem_post EINVAL pid={} rip={:#x}",
            crate::process::current_pid(), crate::arch::syscall::user_rip());
        return 22;
    }
    let atom = unsafe { &*(sem as *const core::sync::atomic::AtomicU32) };
    atom.fetch_add(1, Ordering::Release);
    0
}

// ── TLS ───────────────────────────────────────────────────────────────────

pub fn sys_tls_get_addr(ti_ptr: u64) -> i64 {
    // `ti_ptr` points to a TLS descriptor: [module_id: u64, offset: u64].
    let offset = if ti_ptr != 0 {
        unsafe { *((ti_ptr + 8) as *const u64) }
    } else { 0 };
    let (pid, pml4) = pid_and_pml4();
    let tls_base = get_or_alloc_tls(pid, pml4);
    if tls_base == u64::MAX { return 0; }
    // Return base + (offset & 0x1FFFF) — keep within 128 KiB.
    (tls_base + (offset & 0x1_FFFF)) as i64
}

// ── GNU emulated TLS ──────────────────────────────────────────────────────
//
// Flutter's libflutter_engine.so is commonly built without a native TLS
// segment (no PT_TLS).  The compiler emits calls to __emutls_get_address
// instead of the ELF TLS descriptor protocol.
//
// __emutls_object layout (all fields pointer-sized, i.e. 8 bytes):
//   +0  size   – size of the variable's storage
//   +8  align  – required alignment (unused here, we always 8-align via malloc)
//  +16  ptr    – pointer to allocated storage (0 if not yet allocated)
//  +24  templ  – pointer to initializer template (NULL → zero-init)

pub fn sys_emutls_get_address(obj: u64) -> i64 {
    if obj == 0 { return 0; }
    let (pid, _pml4) = pid_and_pml4();

    // Per-thread cache lookup. DO NOT consult obj+16 — that cell is in the
    // engine's .data and shared across all threads, which would alias TLS
    // between threads and cause call-thru-NULL crashes.
    {
        let table = EMUTLS_TABLE.lock();
        if let Some(&va) = table.get(&(pid, obj)) {
            return va as i64;
        }
    }

    // Allocate storage for this thread's copy of the TLS variable.
    let size = unsafe { *(obj as *const u64) };
    let sz = size.max(8);
    let ptr_va = sys_malloc(sz) as u64;
    if ptr_va == 0 { return 0; }
    // Copy template or zero-init.
    let templ = unsafe { *((obj + 24) as *const u64) };
    if templ != 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(
                templ as *const u8,
                ptr_va as *mut u8,
                sz as usize,
            );
        }
    } else {
        unsafe { core::ptr::write_bytes(ptr_va as *mut u8, 0, sz as usize); }
    }
    // Insert into per-thread cache. Intentionally DO NOT write obj+16.
    EMUTLS_TABLE.lock().insert((pid, obj), ptr_va);
    ptr_va as i64
}

pub fn sys_emutls_register_common(obj: u64, size: u64, align: u64, templ: u64) -> i64 {
    if obj == 0 { return 0; }
    unsafe {
        *(obj as *mut u64)          = size;
        *((obj + 8)  as *mut u64)   = align;
        *((obj + 16) as *mut u64)   = 0;    // legacy: clear cached ptr (unused now)
        *((obj + 24) as *mut u64)   = templ;
    }
    0
}

// ── System ────────────────────────────────────────────────────────────────

pub fn sys_abort() -> ! {
    // Delegate to sys_exit so we never return into the dead process's
    // user code path. sys_exit performs the context switch to the next
    // runnable process (or halts if none), and is `-> i64` only because
    // it threads through the normal dispatch table; in practice it never
    // returns.
    let pid = crate::process::current_pid();
    log::error!("[sys_abort] pid={} aborting — dumping recent syscalls:", pid);
    super::dump_recent_syscalls(32);
    // Dump timerfd + epoll state to help diagnose event-loop starvation.
    super::dump_event_state();
    // Walk user-mode RBP frames to identify the abort call site.
    super::dump_user_backtrace(16);
    super::sys_exit((-6i64) as u64);
    loop {
        unsafe { core::arch::asm!("sti; hlt; cli", options(nomem, nostack)); }
    }
}

pub fn sys_sysconf(name: i32) -> i64 {
    match name {
        // _SC_PAGESIZE / _SC_PAGE_SIZE
        30 | 47 => 4096,
        // _SC_NPROCESSORS_ONLN / _SC_NPROCESSORS_CONF
        // Report 1 CPU: the Dart VM scales JIT worker thread count by CPU
        // count. With 2 CPUs it spawns 2 parallel JIT workers (pids 8 and 9)
        // that race on the class table, triggering ASSERT(previous_cid <
        // current_cid) in il.cc. Under our single-CPU cooperative scheduler
        // with a single BSP, 1 worker is both correct and sufficient.
        84 | 83 => 1,
        // _SC_PHYS_PAGES
        85 => 131072, // 512 MiB / 4096
        // _SC_CLK_TCK
        2 => 100,
        // _SC_OPEN_MAX
        4 => 256,
        // _SC_GETPW_R_SIZE_MAX
        70 => 1024,
        // _SC_GETGR_R_SIZE_MAX
        69 => 1024,
        _ => -1,
    }
}

pub fn sys_nanosleep(req: u64, _rem: u64) -> i64 {
    if req == 0 { return 0; }
    // Read tv_sec from req (offset 0) and tv_nsec (offset 8).
    let secs = unsafe { *(req as *const u64) };
    // Yield for approximately secs * 1000 iterations (very rough).
    for _ in 0..(secs * 1000).min(10000) {
        unsafe { core::arch::asm!("pause", options(nomem, nostack, preserves_flags)); }
    }
    0
}

/// Fake monotonic time: use TSC / 1000 as nanoseconds.
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack)); }
    ((hi as u64) << 32) | lo as u64
}

pub fn sys_gettimeofday(tv: u64, _tz: u64) -> i64 {
    if tv == 0 { return 0; }
    // tv_sec at offset 0, tv_usec at offset 8.
    let tsc = read_tsc() / 3_000; // ~3 GHz TSC → microseconds
    let sec  = 1_700_000_000u64 + tsc / 1_000_000;
    let usec = tsc % 1_000_000;
    unsafe {
        *(tv as *mut u64)       = sec;
        *((tv + 8) as *mut u64) = usec;
    }
    0
}

pub fn sys_clock_gettime(clock_id: i32, tp: u64) -> i64 {
    if tp == 0 { return -22; }
    let tsc_ns = read_tsc() / 3; // ~3 GHz → nanoseconds
    let (sec, nsec) = match clock_id {
        0 | 1 => { // CLOCK_REALTIME or CLOCK_MONOTONIC
            let secs = 1_700_000_000u64 + tsc_ns / 1_000_000_000;
            let ns   = tsc_ns % 1_000_000_000;
            (secs, ns)
        }
        _ => (0, tsc_ns),
    };
    unsafe {
        *(tp as *mut u64)       = sec;
        *((tp + 8) as *mut u64) = nsec;
    }
    0
}

pub fn sys_time(tloc: u64) -> i64 {
    let tsc = read_tsc() / 3_000_000_000; // seconds
    let t = 1_700_000_000u64 + tsc;
    if tloc != 0 { unsafe { *(tloc as *mut u64) = t; } }
    t as i64
}

pub fn sys_getrusage(_who: u64, usage: u64) -> i64 {
    if usage != 0 { unsafe { core::ptr::write_bytes(usage as *mut u8, 0, 144); } }
    0
}

pub fn sys_getcwd(buf: u64, size: u64) -> i64 {
    let path = b"/\0";
    if buf == 0 || size < 2 { return 0; }
    unsafe { core::ptr::copy_nonoverlapping(path.as_ptr(), buf as *mut u8, 2); }
    buf as i64
}

pub fn sys_gethostname(buf: u64, size: u64) -> i64 {
    let name = b"oscortex\0";
    let n = name.len().min(size as usize);
    if buf == 0 { return -1; }
    unsafe { core::ptr::copy_nonoverlapping(name.as_ptr(), buf as *mut u8, n); }
    0
}

pub fn sys_uname(buf: u64) -> i64 {
    // struct utsname: 6 fields × 65 bytes each = 390 bytes.
    if buf == 0 { return -1; }
    unsafe { core::ptr::write_bytes(buf as *mut u8, 0, 390); }
    let copy = |off: usize, s: &[u8]| {
        let n = s.len().min(64);
        unsafe { core::ptr::copy_nonoverlapping(s.as_ptr(), (buf + off as u64) as *mut u8, n); }
    };
    copy(0,   b"Linux");
    copy(65,  b"oscortex");
    copy(130, b"6.1.0");
    copy(195, b"#1 SMP");
    copy(260, b"x86_64");
    0
}

/// Return a static error string for the given errno.
pub fn sys_strerror(errnum: i32) -> i64 {
    log::warn!("[strerror] errnum={}", errnum);
    // Embed static strings at a fixed kernel address.
    // The user gets a pointer to read-only kernel text — valid since
    // user processes can read the kernel text in our current model.
    let s: &'static [u8] = match errnum.unsigned_abs() {
        0  => b"Success\0",
        1  => b"Operation not permitted\0",
        2  => b"No such file or directory\0",
        11 => b"Resource temporarily unavailable\0",
        12 => b"Out of memory\0",
        13 => b"Permission denied\0",
        14 => b"Bad address\0",
        16 => b"Device or resource busy\0", // EBUSY
        17 => b"File exists\0",
        19 => b"No such device\0",
        22 => b"Invalid argument\0",
        28 => b"No space left on device\0",
        35 => b"Resource deadlock would occur\0", // EDEADLK
        38 => b"Function not implemented\0",
        _  => b"Unknown error\0",
    };
    s.as_ptr() as i64
}

pub fn sys_strerror_r(errnum: i32, buf: u64, n: u64) -> i64 {
    log::warn!("[strerror_r] errnum={} buf={:#x} n={}", errnum, buf, n);
    let s_ptr = sys_strerror(errnum) as u64;
    let s_len = sys_strlen(s_ptr) as u64 + 1;
    let copy = s_len.min(n);
    if buf != 0 && copy > 0 {
        sys_memcpy(buf, s_ptr, copy);
    }
    buf as i64
}

pub fn sys_passthrough_syscall(nr: u64, a0: u64, a1: u64, a2: u64) -> i64 {
    // Allow userspace `syscall(NR, ...)` to dispatch to our kernel.
    crate::syscall::dispatch_fast(nr, a0, a1, a2, 0, 0)
}

pub fn sys_getenv(_name_ptr: u64, _name_len: u64) -> i64 {
    // No environment variables in our OS.
    0 // NULL
}

fn ensure_locale_obj() -> u64 {
    let cur = LOCALE_OBJ.load(Ordering::Acquire);
    if cur != 0 { return cur; }

    let obj = sys_malloc(256) as u64;
    if obj == 0 { return 0; }
    unsafe {
        core::ptr::write_bytes(obj as *mut u8, 0, 256);
        *(obj as *mut u64) = obj;
        *((obj + 0x68) as *mut u64) = obj;
    }

    match LOCALE_OBJ.compare_exchange(0, obj, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {
            LOCALE_CURRENT.store(obj, Ordering::Release);
            obj
        }
        Err(existing) => existing,
    }
}

pub fn sys_setlocale(_category: u64, _locale_ptr: u64) -> i64 {
    // Return pointer to a stable locale string "C" in user space.
    crate::process::posix_trampolines::SD_LOCALE_C as i64
}

pub fn sys_uselocale(locale: u64) -> i64 {
    let prev = LOCALE_CURRENT.load(Ordering::Acquire);
    // locale_t special values:
    //   0   => query current locale
    //   -1  => use global locale
    let use_ptr = if locale == 0 || locale == u64::MAX {
        ensure_locale_obj()
    } else {
        locale
    };
    if use_ptr != 0 {
        LOCALE_CURRENT.store(use_ptr, Ordering::Release);
    }
    if prev != 0 { prev as i64 } else { ensure_locale_obj() as i64 }
}

pub fn sys_newlocale(_mask: u64, _name_ptr: u64, base: u64) -> i64 {
    // Some runtimes pass small sentinels here that are not real locale_t
    // pointers; only trust clearly pointer-like values.
    if base >= 0x10000 { return base as i64; }
    ensure_locale_obj() as i64
}

pub fn sys_freelocale(_locale: u64) -> i64 {
    // Locale objects are process-lifetime for now.
    0
}

// ── IO ────────────────────────────────────────────────────────────────────

pub fn sys_open64(path_ptr: u64, _flags: u64, _mode: u64, _arg3: u64) -> i64 {
    // libc open64(path, flags, mode): arg1 is flags, NOT a path length.
    // Compute strlen ourselves so sys_open can read the user path.
    if path_ptr == 0 { return -14; } // EFAULT
    let path_len = sys_strlen(path_ptr) as u64 + 1; // include NUL
    crate::syscall::dispatch_fast(2, path_ptr, path_len, 0, 0, 0)
}

pub fn sys_lseek64(fd: u64, offset: i64, whence: i32) -> i64 {
    crate::syscall::dispatch_fast(8, fd, offset as u64, whence as u64, 0, 0)
}

pub fn sys_fopen64(path_ptr: u64, path_len: u64, _mode_ptr: u64, _mode_len: u64) -> i64 {
    // Open the VFS file via sys_open and return a tiny heap-allocated FILE*.
    // FILE layout: [i64 fd][u64 size][u64 pos]  = 24 bytes
    let path_end = if path_len > 0 { path_len } else { sys_strlen(path_ptr) as u64 + 1 };
    let fd = crate::syscall::dispatch_fast(2, path_ptr, path_end, 0, 0, 0);
    if fd < 0 { return 0; } // NULL on error
    // Query file size via fstat.
    let stat_buf = sys_malloc(144 + 16);
    if stat_buf == 0 { crate::syscall::dispatch_fast(3, fd as u64, 0, 0, 0, 0); return 0; }
    crate::syscall::dispatch_fast(5, fd as u64, stat_buf as u64, 0, 0, 0); // fstat
    let size = unsafe { *((stat_buf as u64 + 48) as *const u64) }; // st_size offset=48
    sys_free(stat_buf as u64);
    // Allocate FILE struct: [fd:i64][size:u64][pos:u64]
    let fp = sys_malloc(24);
    if fp == 0 { crate::syscall::dispatch_fast(3, fd as u64, 0, 0, 0, 0); return 0; }
    unsafe {
        *((fp as u64) as *mut i64)      = fd;
        *((fp as u64 + 8) as *mut u64)  = size;
        *((fp as u64 + 16) as *mut u64) = 0; // pos
    }
    fp
}

pub fn sys_fclose(fp: u64) -> i64 {
    if fp == 0 { return -1; }
    let fd = unsafe { *(fp as *const i64) };
    sys_free(fp);
    crate::syscall::dispatch_fast(3, fd as u64, 0, 0, 0, 0) // sys_close
}

pub fn sys_fread(buf: u64, size: u64, count: u64, fp: u64) -> i64 {
    if fp == 0 || buf == 0 { return 0; }
    let fd = unsafe { *(fp as *const i64) } as u64;
    let total = size * count;
    let r = crate::syscall::dispatch_fast(0, fd, buf, total, 0, 0); // sys_read
    if r <= 0 { return 0; }
    // Update pos in FILE struct.
    unsafe { *((fp + 16) as *mut u64) += r as u64; }
    if size == 0 { 0 } else { r / size as i64 }
}

pub fn sys_fwrite(buf: u64, size: u64, count: u64, fp: u64) -> i64 {
    let fd: u64 = if fp == 0 { 1 } else { unsafe { *(fp as *const i64) as u64 } };
    let total = size * count;
    let r = crate::syscall::dispatch_fast(1, fd, buf, total, 0, 0);
    if r < 0 { 0 } else { r / size as i64 }
}

pub fn sys_fseek(fp: u64, offset: i64, whence: i32) -> i64 {
    if fp == 0 { return -1; }
    let fd = unsafe { *(fp as *const i64) } as u64;
    let new_pos = crate::syscall::dispatch_fast(8, fd, offset as u64, whence as u64, 0, 0);
    if new_pos < 0 { return -1; }
    unsafe { *((fp + 16) as *mut u64) = new_pos as u64; }
    0
}

pub fn sys_ftell(fp: u64) -> i64 {
    if fp == 0 { return -1; }
    let pos = unsafe { *((fp + 16) as *const u64) };
    pos as i64
}

pub fn sys_fgets(buf: u64, size: i32, fp: u64) -> i64 {
    if fp == 0 || buf == 0 || size <= 0 { return 0; }
    let fd = unsafe { *(fp as *const i64) } as u64;
    let cap = (size - 1).max(0) as u64;
    // Read byte by byte until newline or EOF.
    let mut n: u64 = 0;
    while n < cap {
        let mut ch: u8 = 0;
        let r = crate::syscall::dispatch_fast(0, fd, &mut ch as *mut u8 as u64, 1, 0, 0);
        if r <= 0 { break; }
        unsafe { *((buf + n) as *mut u8) = ch; }
        n += 1;
        if ch == b'\n' { break; }
    }
    unsafe { *((buf + n) as *mut u8) = 0; }
    if n == 0 { 0 } else { buf as i64 }
}

pub fn sys_fileno(fp: u64) -> i64 {
    if fp == 0 { return -1; }
    unsafe { *(fp as *const i64) }
}

pub fn sys_feof(fp: u64) -> i64 {
    if fp == 0 { return 1; }
    let fd  = unsafe { *(fp as *const i64) } as usize;
    let tbl = super::OPEN_FILES.lock();
    if fd < super::MAX_OPEN_FILES && tbl[fd].used {
        if tbl[fd].offset >= tbl[fd].data.len() as u64 { 1 } else { 0 }
    } else {
        1
    }
}

pub fn sys_ferror(_fp: u64) -> i64 { 0 }

pub fn sys_clearerr(_fp: u64) -> i64 { 0 }

pub fn sys_rewind(fp: u64) -> i64 {
    sys_fseek(fp, 0, 0 /* SEEK_SET */)
}

pub fn sys_fflush(_fp: u64) -> i64 { 0 }

pub fn sys_stat(path_ptr: u64, path_len: u64, stat_ptr: u64) -> i64 {
    // The POSIX `stat()` ABI is `stat(const char *path, struct stat *buf)`.
    // When called via the libc trampoline (syscall 0x441), `arg1` is the
    // stat buffer pointer and `arg2` is unused — there is no length. Detect
    // that case by checking whether `stat_ptr` is zero, and treat
    // `path_len` as the real stat buffer.
    let (mut real_path_len, real_stat_ptr) = if stat_ptr == 0 {
        (0u64, path_len)
    } else {
        (path_len, stat_ptr)
    };
    // If no path length was supplied, compute it from the NUL terminator.
    if real_path_len == 0 && path_ptr != 0 {
        let mut len: usize = 0;
        unsafe {
            let p = path_ptr as *const u8;
            while len < 4096 && *p.add(len) != 0 { len += 1; }
        }
        real_path_len = len as u64;
    }
    // Open the file, stat it, close it.
    let fd = crate::syscall::dispatch_fast(2, path_ptr, real_path_len, 0, 0, 0);
    if fd < 0 { return -2; } // ENOENT
    let r = crate::syscall::dispatch_fast(5, fd as u64, real_stat_ptr, 0, 0, 0);
    crate::syscall::dispatch_fast(3, fd as u64, 0, 0, 0, 0);
    r
}

fn format_into(
    buf: u64,
    size: u64,
    fmt_ptr: u64,
    mut next_int: impl FnMut() -> u64,
) -> i64 {
    let fmt_valid = fmt_ptr >= 0x10000 && fmt_ptr < 0x0000_8000_0000_0000;
    if !fmt_valid { return 0; }
    let buf_valid = buf >= 0x10000 && buf < 0x0000_8000_0000_0000;
    if size > 0 && !buf_valid { return 0; }

    let cur_cr3 = crate::arch::memory::read_cr3() & 0x000f_ffff_ffff_f000;

    // Safe C-string reader from user address space.
    let read_cstr = |p: u64, out: &mut [u8]| -> usize {
        if p < 0x10000 || p >= 0x0000_8000_0000_0000 { return 0; }
        if crate::mm::paging::translate_user_page(cur_cr3, p & !0xfff).is_none() { return 0; }
        let mut n = 0usize;
        unsafe {
            let pp = p as *const u8;
            while n < out.len() {
                let c = *pp.add(n);
                if c == 0 { break; }
                out[n] = c;
                n += 1;
            }
        }
        n
    };

    // Read the format string.
    let mut fbuf = [0u8; 256];
    let flen = read_cstr(fmt_ptr, &mut fbuf);
    let fmt = &fbuf[..flen];

    let mut out_buf = [0u8; 512];
    let mut out_len = 0usize;
    let mut total_len = 0usize;
    let mut i = 0usize;

    // Helper to append a slice to the formatted output
    let mut append_bytes = |bytes: &[u8]| {
        let limit = (511usize.saturating_sub(out_len)).min(bytes.len());
        if limit > 0 {
            out_buf[out_len..out_len + limit].copy_from_slice(&bytes[..limit]);
            out_len += limit;
        }
        total_len += bytes.len();
    };

    while i < flen {
        let c = fmt[i];
        if c != b'%' || i + 1 >= flen {
            append_bytes(&[c]);
            i += 1;
            continue;
        }

        // Skip format spec modifiers and consume variable width/precision arguments
        let mut j = i + 1;
        while j < flen && matches!(fmt[j], b'l'|b'h'|b'z'|b'j'|b't'|b'L'|b'0'..=b'9'|b'.'|b'-'|b'+'|b' '|b'#'|b'*') {
            if fmt[j] == b'*' {
                let _ = next_int();
            }
            j += 1;
        }
        if j >= flen { break; }
        let conv = fmt[j];
        let mut tmp = [0u8; 32];
        match conv {
            b'd' | b'i' => {
                let v = next_int() as i64;
                let mut n = 0usize;
                let (neg, mut u) = if v < 0 { (true, (v.wrapping_neg()) as u64) } else { (false, v as u64) };
                if u == 0 { tmp[n] = b'0'; n += 1; }
                while u > 0 { tmp[n] = b'0' + (u % 10) as u8; n += 1; u /= 10; }
                if neg { tmp[n] = b'-'; n += 1; }
                tmp[..n].reverse();
                append_bytes(&tmp[..n]);
            }
            b'u' => {
                let mut u = next_int();
                let mut n = 0usize;
                if u == 0 { tmp[n] = b'0'; n += 1; }
                while u > 0 { tmp[n] = b'0' + (u % 10) as u8; n += 1; u /= 10; }
                tmp[..n].reverse();
                append_bytes(&tmp[..n]);
            }
            b'x' | b'X' | b'p' => {
                let mut u = next_int();
                if conv == b'p' {
                    append_bytes(b"0x");
                }
                let hexd = if conv == b'X' { b"0123456789ABCDEF" } else { b"0123456789abcdef" };
                let mut n = 0usize;
                if u == 0 { tmp[n] = b'0'; n += 1; }
                while u > 0 { tmp[n] = hexd[(u & 0xF) as usize]; n += 1; u >>= 4; }
                tmp[..n].reverse();
                append_bytes(&tmp[..n]);
            }
            b's' => {
                let p = next_int();
                let mut sbuf = [0u8; 256];
                let sn = read_cstr(p, &mut sbuf);
                append_bytes(&sbuf[..sn]);
            }
            b'c' => {
                let v = next_int() as u8;
                append_bytes(&[v]);
            }
            b'%' => {
                append_bytes(b"%");
            }
            _ => {
                append_bytes(&[b'%', conv]);
            }
        }
        i = j + 1;
    }

    let fmt_str = core::str::from_utf8(fmt).unwrap_or("<non-utf8>");
    let is_error = flen >= 5 && (fmt_str.contains("error") || fmt_str.contains("expected") || fmt_str.contains("assert") || fmt_str.contains("fail") || fmt_str.contains("FATAL"));
    if is_error {
        static SNPRINTF_ERR_COUNT: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let n = SNPRINTF_ERR_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 16 {
            let msg = core::str::from_utf8(&out_buf[..out_len]).unwrap_or("<non-utf8>");
            log::error!("[snprintf-error] pid={} #{}: msg=\"{}\" fmt=\"{}\"",
                crate::process::current_pid(), n, msg, fmt_str);
        }
    }

    // Write formatted result to the caller's buffer.
    if size > 0 && buf_valid {
        let copy_len = (size as usize - 1).min(out_len);
        unsafe {
            core::ptr::copy_nonoverlapping(out_buf.as_ptr(), buf as *mut u8, copy_len);
            core::ptr::write((buf as *mut u8).add(copy_len), 0);
        }
    }
    total_len as i64
}

pub fn sys_snprintf(buf: u64, size: u64, fmt_ptr: u64, first_vararg: u64, second_vararg: u64) -> i64 {
    let user_rsp = crate::arch::syscall::user_rsp();
    let third_vararg  = crate::arch::syscall::user_r9();
    let cur_cr3 = crate::arch::memory::read_cr3() & 0x000f_ffff_ffff_f000;
    let safe_qword = |off: u64| -> u64 {
        let addr = user_rsp.wrapping_add(off);
        if crate::mm::paging::translate_user_page(cur_cr3, addr & !0xfff).is_some() {
            unsafe { core::ptr::read_volatile(addr as *const u64) }
        } else { 0 }
    };
    let fourth_vararg = safe_qword(8);
    let fifth_vararg  = safe_qword(16);
    let varargs = [first_vararg, second_vararg, third_vararg, fourth_vararg, fifth_vararg];
    let mut varg_idx = 0usize;
    format_into(buf, size, fmt_ptr, || {
        let val = if varg_idx < varargs.len() { varargs[varg_idx] } else { 0 };
        varg_idx += 1;
        val
    })
}

pub fn sys_vsnprintf(buf: u64, size: u64, fmt_ptr: u64, ap: u64) -> i64 {
    let plausible_user = |p: u64| p != 0 && p < 0x0000_8000_0000_0000;
    let (mut gp_off, reg_save, mut ovf): (u32, u64, u64) =
        if plausible_user(ap) {
            unsafe {
                let g = core::ptr::read_unaligned((ap as *const u32));
                let r = core::ptr::read_unaligned((ap as *const u64).offset(2));
                let o = core::ptr::read_unaligned((ap as *const u64).offset(1));
                (g, r, o)
            }
        } else { (48, 0, 0) };

    static VSNPRINTF_DEBUG_LOG: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    let dbg_n = VSNPRINTF_DEBUG_LOG.fetch_add(1, Ordering::Relaxed);
    if dbg_n < 8 {
        log::warn!(
            "[vsnprintf-debug] #{} pid={} ap={:#x} gp_off={} reg_save={:#x} ovf={:#x}",
            dbg_n,
            crate::process::current_pid(),
            ap,
            gp_off,
            reg_save,
            ovf
        );
    }

    let mut next_int = || -> u64 {
        let val = if gp_off < 48 && plausible_user(reg_save) {
            let v = unsafe { core::ptr::read_unaligned((reg_save + gp_off as u64) as *const u64) };
            gp_off += 8;
            v
        } else if plausible_user(ovf) {
            let v = unsafe { core::ptr::read_unaligned(ovf as *const u64) };
            ovf += 8;
            v
        } else { 0 };
        if dbg_n < 8 {
            log::warn!("[vsnprintf-debug] arg={:#x}", val);
        }
        val
    };

    format_into(buf, size, fmt_ptr, next_int)
}

pub fn sys_printf(fmt_ptr: u64, _fmt_len: u64) -> i64 {
    let len = sys_strlen(fmt_ptr) as u64;
    crate::syscall::dispatch_fast(1, 1, fmt_ptr, len, 0, 0)
}

pub fn sys_puts(s: u64) -> i64 {
    let len = sys_strlen(s) as u64;
    crate::syscall::dispatch_fast(1, 1, s, len, 0, 0);
    crate::syscall::dispatch_fast(1, 1, b"\n".as_ptr() as u64, 1, 0, 0);
    0
}

pub fn sys_perror(msg: u64) -> i64 {
    // Log caller's return address (at top of user stack) to identify flutter call site.
    {
        static PERROR_TRACE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        let n = PERROR_TRACE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 4 {
            let rsp = crate::arch::syscall::user_rsp();
            let caller_rip = unsafe { *(rsp as *const u64) };
            let epoll_ret_rax = crate::process::get_saved_rax(crate::process::current_pid());
            log::warn!("[sys_perror] #{} pid={} msg={:#x} caller_ret={:#x} prev_epoll_rax={}",
                n, crate::process::current_pid(), msg, caller_rip, epoll_ret_rax as i64);
        }
    }
    if msg != 0 {
        let mlen = sys_strlen(msg) as u64;
        crate::syscall::dispatch_fast(1, 2, msg, mlen, 0, 0);
        crate::syscall::dispatch_fast(1, 2, b": ".as_ptr() as u64, 2, 0, 0);
    }
    // Write ENOSYS strerror.
    let s = sys_strerror(38) as u64;
    let slen = sys_strlen(s) as u64;
    crate::syscall::dispatch_fast(1, 2, s, slen, 0, 0);
    crate::syscall::dispatch_fast(1, 2, b"\n".as_ptr() as u64, 1, 0, 0);
    0
}

pub fn sys_fprintf(fp: u64, fmt_ptr: u64, ap: u64) -> i64 {
    // Simplified vfprintf with minimal %i / %s / %d / %x expansion so
    // diagnostic messages (errno, strerror) are actually readable.
    //
    // C semantics for both fprintf(stream, fmt, ...) and vfprintf(stream,
    // fmt, va_list ap): the format string is NUL-terminated and the third
    // ABI slot is a va_list pointer — never a byte count. Earlier code
    // (mis-)used arg2 as a length, which made vfprintf hand its caller's
    // stack pointer to sys_write as the buffer length, then EFAULT, then
    // abort.
    //
    // SysV x86_64 va_list layout (struct __va_list_tag):
    //   off 0:   u32 gp_offset      (next int-reg byte offset 0..48)
    //   off 4:   u32 fp_offset
    //   off 8:   *mut u8 overflow_arg_area
    //   off 16:  *mut u8 reg_save_area
    if fmt_ptr == 0 { return 0; }
    let fd: u64 = if fp == 0 || fp == 1 { 1 }
                  else if fp == 2 { 2 }
                  else { unsafe { *(fp as *const i64) as u64 } };

    // Read va_list state if pointer is plausibly a user-mode pointer.
    let plausible_user = |p: u64| p != 0 && p < 0x0000_8000_0000_0000;
    let (mut gp_off, reg_save, mut ovf): (u32, u64, u64) =
        if plausible_user(ap) {
            unsafe {
                let g = core::ptr::read_unaligned((ap as *const u32));
                let r = core::ptr::read_unaligned((ap as *const u64).offset(2));
                let o = core::ptr::read_unaligned((ap as *const u64).offset(1));
                (g, r, o)
            }
        } else { (48, 0, 0) };

    // Pull next u64-sized int arg from the va_list. Integers consume
    // 8 bytes of gp regs first, then overflow.
    let mut next_int = || -> u64 {
        if gp_off < 48 && plausible_user(reg_save) {
            let v = unsafe { core::ptr::read_unaligned((reg_save + gp_off as u64) as *const u64) };
            gp_off += 8;
            v
        } else if plausible_user(ovf) {
            let v = unsafe { core::ptr::read_unaligned(ovf as *const u64) };
            ovf += 8;
            v
        } else { 0 }
    };

    // Format into a stack buffer (cap 512 bytes) then sys_write.
    let mut out = [0u8; 512];
    let mut olen: usize = 0;
    let mut push = |s: &[u8], olen: &mut usize, out: &mut [u8; 512]| {
        let n = s.len().min(out.len() - *olen);
        out[*olen..*olen + n].copy_from_slice(&s[..n]);
        *olen += n;
    };

    let fmt_len = sys_strlen(fmt_ptr) as usize;
    let fmt = unsafe { core::slice::from_raw_parts(fmt_ptr as *const u8, fmt_len) };
    let mut i = 0;
    while i < fmt.len() && olen < out.len() {
        let c = fmt[i];
        if c != b'%' || i + 1 >= fmt.len() {
            push(&[c], &mut olen, &mut out);
            i += 1;
            continue;
        }
        // Skip flags/width — minimal: read a single conversion char,
        // ignoring length modifiers ('l','ll','h','z') and consuming variable arguments for '*'.
        let mut j = i + 1;
        while j < fmt.len() && matches!(fmt[j], b'l'|b'h'|b'z'|b'j'|b't'|b'L'|b'0'..=b'9'|b'.'|b'-'|b'+'|b' '|b'#'|b'*') {
            if fmt[j] == b'*' {
                let _ = next_int();
            }
            j += 1;
        }
        if j >= fmt.len() { break; }
        let conv = fmt[j];
        let mut tmp = [0u8; 32];
        match conv {
            b'd' | b'i' => {
                let v = next_int() as i64;
                let mut n = 0usize;
                let (neg, mut u) = if v < 0 { (true, (v.wrapping_neg()) as u64) } else { (false, v as u64) };
                if u == 0 { tmp[n] = b'0'; n += 1; }
                while u > 0 { tmp[n] = b'0' + (u % 10) as u8; n += 1; u /= 10; }
                if neg { tmp[n] = b'-'; n += 1; }
                tmp[..n].reverse();
                push(&tmp[..n], &mut olen, &mut out);
            }
            b'u' => {
                let mut u = next_int();
                let mut n = 0usize;
                if u == 0 { tmp[n] = b'0'; n += 1; }
                while u > 0 { tmp[n] = b'0' + (u % 10) as u8; n += 1; u /= 10; }
                tmp[..n].reverse();
                push(&tmp[..n], &mut olen, &mut out);
            }
            b'x' | b'X' | b'p' => {
                let mut u = next_int();
                if conv == b'p' { push(b"0x", &mut olen, &mut out); }
                let hexd = if conv == b'X' { b"0123456789ABCDEF" } else { b"0123456789abcdef" };
                let mut n = 0usize;
                if u == 0 { tmp[n] = b'0'; n += 1; }
                while u > 0 { tmp[n] = hexd[(u & 0xF) as usize]; n += 1; u >>= 4; }
                tmp[..n].reverse();
                push(&tmp[..n], &mut olen, &mut out);
            }
            b's' => {
                let p = next_int();
                if plausible_user(p) {
                    let sl = sys_strlen(p) as usize;
                    let s = unsafe { core::slice::from_raw_parts(p as *const u8, sl) };
                    push(s, &mut olen, &mut out);
                } else {
                    push(b"(null)", &mut olen, &mut out);
                }
            }
            b'c' => {
                let v = next_int() as u8;
                push(&[v], &mut olen, &mut out);
            }
            b'%' => { push(b"%", &mut olen, &mut out); }
            _ => {
                // Unknown — emit literally.
                push(&fmt[i..=j], &mut olen, &mut out);
            }
        }
        i = j + 1;
    }
    if olen == 0 { return 0; }
    crate::syscall::dispatch_fast(1, fd, out.as_ptr() as u64, olen as u64, 0, 0)
}

// ── Signals ───────────────────────────────────────────────────────────────

pub fn sys_sigfillset(set: u64) -> i64 {
    // Fill all bits in sigset_t (128 bytes on Linux x86_64).
    if set != 0 { unsafe { core::ptr::write_bytes(set as *mut u8, 0xFF, 128); } }
    0
}

// ── Wide chars ────────────────────────────────────────────────────────────

pub fn sys_wcslen(s: u64) -> i64 {
    // wchar_t is 4 bytes on Linux.
    if s == 0 { return 0; }
    let mut len = 0i64;
    let mut p = s as *const u32;
    unsafe { while *p != 0 { p = p.add(1); len += 1; } }
    len
}

pub fn sys_mbrtowc(pwc: u64, s: u64, n: u64, _ps: u64) -> i64 {
    // C locale: 1-byte encoding.
    if s == 0 { return 0; }
    if n == 0 { return -2; } // incomplete sequence
    let b = unsafe { *(s as *const u8) };
    if b == 0 {
        if pwc != 0 { unsafe { *(pwc as *mut u32) = 0; } }
        return 0;
    }
    if pwc != 0 { unsafe { *(pwc as *mut u32) = b as u32; } }
    1
}

pub fn sys_mbsnrtowcs(dst: u64, srcp: u64, nmc: u64, len: u64, _ps: u64) -> i64 {
    if srcp == 0 { return -1; }
    let src_ptr = unsafe { *(srcp as *const u64) };
    if src_ptr == 0 { return 0; }

    let mut src = src_ptr as *const u8;
    let mut converted: u64 = 0;
    let mut remaining = nmc;
    while remaining > 0 {
        let b = unsafe { *src };
        if b == 0 {
            unsafe { *(srcp as *mut u64) = 0; }
            break;
        }
        if dst != 0 {
            if converted >= len { break; }
            unsafe { *((dst as *mut u32).add(converted as usize)) = b as u32; }
        }
        src = unsafe { src.add(1) };
        converted += 1;
        remaining -= 1;
    }

    if unsafe { *src } != 0 {
        unsafe { *(srcp as *mut u64) = src as u64; }
    }
    converted as i64
}

pub fn sys_mbsrtowcs(dst: u64, srcp: u64, len: u64, ps: u64) -> i64 {
    // Unbounded input bytes for C locale path.
    sys_mbsnrtowcs(dst, srcp, u64::MAX / 2, len, ps)
}

pub fn sys_mbtowc(pwc: u64, s: u64, n: u64) -> i64 {
    if s == 0 { return 0; } // reset shift state
    sys_mbrtowc(pwc, s, n, 0)
}

pub fn sys_wcrtomb(s: u64, wc: u64, _ps: u64) -> i64 {
    // C locale: only single-byte characters.
    if s == 0 { return 1; } // state reset query
    let ch = wc as u32;
    if ch > 0xFF { return -1; }
    unsafe { *(s as *mut u8) = ch as u8; }
    1
}

pub fn sys_wmemchr(s: u64, wc: u64, n: u64) -> i64 {
    if s == 0 || n == 0 { return 0; }
    let target = wc as u32;
    let p = s as *const u32;
    for i in 0..(n as usize) {
        let cur = unsafe { *p.add(i) };
        if cur == target {
            return (s + (i as u64) * 4) as i64;
        }
    }
    0
}

// ── Futex helpers ─────────────────────────────────────────────────────────

pub fn sys_futex_wake(addr: u64, count: u32) -> i64 {
    // Dispatch to the existing futex syscall.
    crate::syscall::dispatch_fast(0x39D, addr, 129, count as u64, 0, 0) // FUTEX_WAKE = 1
}

