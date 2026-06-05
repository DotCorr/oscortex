//! POSIX Compatibility Trampoline Pages
//!
//! Maps fixed user-space pages into every new process providing:
//!  - TRAMPOLINE_PAGES_VA (2 pages): 16-byte syscall stubs for every glibc symbol
//!    the Flutter engine needs, via `syscall` instruction.
//!  - SYSDATA_VA (1 page): static data for environ, FILE* handles, ctype tables,
//!    errno storage, etc.
//!
//! dl.rs resolves SHN_UNDEF symbols from libflutter_engine.so by looking up the
//! symbol name in POSIX_SYMBOLS and patching the GOT entry with the VA in the
//! trampoline page. At runtime the trampoline does `mov eax, NR; syscall; ret`.
//!
//! Special stubs:
//!  - FloatZero: `xorps xmm0,xmm0; ret; nop*12`  — for math functions
//!  - RetAddr:   `movabs rax, IMM64; ret; nop*5`  — ctype/errno location funcs
//!  - RetU32:    `mov eax, IMM32; ret; nop*10`    — getpagesize, mb_cur_max, etc.
//!
//! Data symbols (environ/stdin/stdout/stderr) resolve directly to SYSDATA offsets.

use crate::mm::{frame_allocator, paging};

// ── Virtual address constants ─────────────────────────────────────────────────

/// First of 2 contiguous trampoline pages (0x7FFF_E000 – 0x7FFF_FFFF).
/// VA(i) = TRAMPOLINE_PAGES_VA + i * 16.  Max 512 stubs.
pub const TRAMPOLINE_PAGES_VA: u64 = 0x7FFF_E000;

/// 1-page system data region (0x7FFF_C000 – 0x7FFF_CFFF).
pub const SYSDATA_VA: u64 = 0x7FFF_C000;

/// VA of the internal "thread return" trampoline.
/// Encoded as `mov rdi, rax; mov eax, 0x35B; syscall; ud2`, so when a thread
/// start routine returns and `ret`s into this slot, its RAX return value
/// becomes the thread exit code.
///
/// Computed by scanning STUBS for the well-known internal symbol name.
/// Returns 0 if the symbol is missing (should never happen).
pub fn thread_return_trampoline_va() -> u64 {
    find_posix_symbol("__oscortex_thread_return").unwrap_or(0)
}

// Offsets within SYSDATA page:
pub const SD_CTYPE_B_PTR:   u64 = SYSDATA_VA + 0;   // u64 → &ctype_b_table[128]
pub const SD_CTYPE_LO_PTR:  u64 = SYSDATA_VA + 8;   // u64 → &ctype_lo_table[128]
pub const SD_CTYPE_UP_PTR:  u64 = SYSDATA_VA + 16;  // u64 → &ctype_up_table[128]
// offset 24: reserved
pub const SD_ERRNO:         u64 = SYSDATA_VA + 32;  // u64: per-process errno
// offset 40: reserved
pub const SD_ENVIRON:       u64 = SYSDATA_VA + 48;  // u64: null char** (environ)
pub const SD_STDIN:         u64 = SYSDATA_VA + 56;  // u64: null FILE*
pub const SD_STDOUT:        u64 = SYSDATA_VA + 64;  // u64: null FILE*
pub const SD_STDERR:        u64 = SYSDATA_VA + 72;  // u64: null FILE*
// ctype tables start at offset 80:
const SD_CTYPE_B_TABLE:   u64 = SYSDATA_VA + 80;   // u16[384] = 768 bytes
const SD_CTYPE_LO_TABLE:  u64 = SYSDATA_VA + 848;  // i32[384] = 1536 bytes
const SD_CTYPE_UP_TABLE:  u64 = SYSDATA_VA + 2384; // i32[384] = 1536 bytes
// Total: 2384 + 1536 = 3920 bytes < 4096 ✓
pub const SD_LOCALE_C: u64 = SYSDATA_VA + 3920;
pub const SD_EMPTY_FC_FONTSET: u64 = SYSDATA_VA + 4032;

// Pointer values written into sysdata page at init time:
const CTYPE_B_PTR_VAL:  u64 = SD_CTYPE_B_TABLE  + 128 * 2; // &ctype_b_table[128]
const CTYPE_LO_PTR_VAL: u64 = SD_CTYPE_LO_TABLE + 128 * 4; // &ctype_lo_table[128]
const CTYPE_UP_PTR_VAL: u64 = SD_CTYPE_UP_TABLE + 128 * 4; // &ctype_up_table[128]

// ── Stub kind ─────────────────────────────────────────────────────────────────

/// How to encode a 16-byte trampoline entry.
#[derive(Clone, Copy)]
enum StubKind {
    /// mov r10,rcx; mov eax,NR; syscall; ret; nop*5
    Syscall(u32),
    /// xorps xmm0,xmm0; ret; nop*12  (math functions returning 0.0)
    FloatZero,
    /// movabs rax, ADDR; ret; nop*5  (functions returning a fixed address)
    RetAddr(u64),
    /// mov eax, VAL; ret; nop*10  (functions returning a small constant)
    RetU32(u32),
    /// mov rdi, rax; mov eax, NR; syscall; ud2; nop*5
    /// Used for noreturn syscalls whose first arg should be the caller's
    /// return value (e.g. pthread_exit invoked implicitly via `ret` from the
    /// thread start routine — RAX holds the routine's return value).
    SyscallRetAsArg0(u32),
    /// Custom inline userspace qsort implementation (136 bytes, spans 9 slots)
    Qsort,
    /// Padding for multi-slot custom code (noop during encoding)
    Padding,
    /// Custom inline userspace _setjmp implementation (44 bytes, spans 3 slots)
    Setjmp,
    /// Custom inline userspace longjmp implementation (48 bytes, spans 3 slots)
    Longjmp,
    /// Unary libm call: `movq rdi,xmm0; mov eax,NR; syscall; movq xmm0,rax; ret`
    /// (18 bytes — spans 2 slots). Routes to kernel `libm_call` via syscall NR.
    MathSyscall1(u32),
    /// Binary libm call: passes xmm0→rdi, xmm1→rsi (23 bytes — spans 2 slots).
    MathSyscall2(u32),
    /// Ternary libm call (fma): xmm0→rdi, xmm1→rsi, xmm2→rdx (28 bytes — 2 slots).
    MathSyscall3(u32),
}

// ── Master symbol list ────────────────────────────────────────────────────────
//
// Every entry either:
//   (a) FunctionStub(idx) → VA = TRAMPOLINE_PAGES_VA + idx * 16
//   (b) DataSym(va)       → VA supplied directly (in SYSDATA page)

// Assign sequential indices to function stubs. Indices are embedded in the
// POSIX_SYMBOLS table below.
//
// Stub layout in trampoline page:
//   offset = idx * 16

const STUBS: &[(&str, StubKind)] = &[
    // ── Memory allocation ──────────────────────────────────────────────────
    /* 0  */ ("malloc",               StubKind::Syscall(0x3A0)),
    /* 1  */ ("free",                 StubKind::Syscall(0x3A1)),
    /* 2  */ ("calloc",               StubKind::Syscall(0x3A2)),
    /* 3  */ ("realloc",              StubKind::Syscall(0x3A3)),
    /* 4  */ ("aligned_alloc",        StubKind::Syscall(0x3A4)),
    /* 5  */ ("posix_memalign",       StubKind::Syscall(0x3A5)),
    /* 6  */ ("malloc_usable_size",   StubKind::Syscall(0x3A6)),
    /* 7  */ ("strdup",               StubKind::Syscall(0x3A7)),
    /* 8  */ ("strndup",              StubKind::Syscall(0x3A8)),
    // ── String operations ──────────────────────────────────────────────────
    /* 9  */ ("strlen",               StubKind::Syscall(0x3B0)),
    /* 10 */ ("memcpy",               StubKind::Syscall(0x3B1)),
    /* 11 */ ("memset",               StubKind::Syscall(0x3B2)),
    /* 12 */ ("memmove",              StubKind::Syscall(0x3B3)),
    /* 13 */ ("memcmp",               StubKind::Syscall(0x3B4)),
    /* 14 */ ("memchr",               StubKind::Syscall(0x3B5)),
    /* 15 */ ("bcmp",                 StubKind::Syscall(0x3B4)), // alias memcmp
    /* 16 */ ("bzero",                StubKind::Syscall(0x3B6)),
    /* 17 */ ("strcmp",               StubKind::Syscall(0x3B7)),
    /* 18 */ ("strncmp",              StubKind::Syscall(0x3B8)),
    /* 19 */ ("strcpy",               StubKind::Syscall(0x3B9)),
    /* 20 */ ("strncpy",              StubKind::Syscall(0x3BA)),
    /* 21 */ ("strcat",               StubKind::Syscall(0x3BB)),
    /* 22 */ ("strncat",              StubKind::Syscall(0x3BC)),
    /* 23 */ ("strstr",               StubKind::Syscall(0x3BD)),
    /* 24 */ ("strchr",               StubKind::Syscall(0x3BE)),
    /* 25 */ ("strrchr",              StubKind::Syscall(0x3BF)),
    /* 26 */ ("strnlen",              StubKind::Syscall(0x3C0)),
    /* 27 */ ("strcspn",              StubKind::Syscall(0x3C1)),
    /* 28 */ ("strspn",               StubKind::Syscall(0x3C2)),
    /* 29 */ ("strtok_r",             StubKind::Syscall(0x3C3)),
    /* 30 */ ("strcasestr",           StubKind::Syscall(0x3C4)),
    /* 31 */ ("strtol",               StubKind::Syscall(0x3C5)),
    /* 32 */ ("strtoul",              StubKind::Syscall(0x3C6)),
    /* 33 */ ("strtoll",              StubKind::Syscall(0x3C7)),
    /* 34 */ ("strtoull",             StubKind::Syscall(0x3C8)),
    /* 35 */ ("strtod",               StubKind::Syscall(0x3C9)),
    /* 36 */ ("atoi",                 StubKind::Syscall(0x3CA)),
    /* 37 */ ("atol",                 StubKind::Syscall(0x3CA)),  // alias atoi
    /* 38 */ ("__qsort_old",          StubKind::Syscall(0x3CB)),
    /* 39 */ ("rand",                 StubKind::Syscall(0x3CC)),
    /* 40 */ ("srand",                StubKind::Syscall(0x3CD)),
    // ── Threading ──────────────────────────────────────────────────────────
    /* 41 */ ("pthread_create",       StubKind::Syscall(0x35A)), // existing
    /* 42 */ ("pthread_join",         StubKind::Syscall(0x35C)), // existing
    /* 43 */ ("pthread_exit",         StubKind::Syscall(0x35B)), // existing
    /* 44 */ ("pthread_detach",       StubKind::Syscall(0x3D0)),
    /* 45 */ ("pthread_self",         StubKind::Syscall(0x3D1)),
    /* 46 */ ("pthread_mutex_init",   StubKind::Syscall(0x3D2)),
    /* 47 */ ("pthread_mutex_lock",   StubKind::Syscall(0x3D3)),
    /* 48 */ ("pthread_mutex_unlock", StubKind::Syscall(0x3D4)),
    /* 49 */ ("pthread_mutex_destroy",StubKind::Syscall(0x3D5)),
    /* 50 */ ("pthread_mutex_trylock",StubKind::Syscall(0x3D6)),
    /* 51 */ ("pthread_once",         StubKind::Syscall(0x3D7)),
    /* 52 */ ("pthread_key_create",   StubKind::Syscall(0x3D8)),
    /* 53 */ ("pthread_key_delete",   StubKind::Syscall(0x3D9)),
    /* 54 */ ("pthread_setspecific",  StubKind::Syscall(0x3DA)),
    /* 55 */ ("pthread_getspecific",  StubKind::Syscall(0x3DB)),
    /* 56 */ ("pthread_cond_init",    StubKind::Syscall(0x3DC)),
    /* 57 */ ("pthread_cond_wait",    StubKind::Syscall(0x3DD)),
    /* 58 */ ("pthread_cond_signal",  StubKind::Syscall(0x3DE)),
    /* 59 */ ("pthread_cond_broadcast",StubKind::Syscall(0x3DF)),
    /* 60 */ ("pthread_cond_destroy", StubKind::Syscall(0x3E0)),
    /* 61 */ ("pthread_cond_timedwait",StubKind::Syscall(0x3E1)),
    /* 62 */ ("pthread_rwlock_rdlock",StubKind::Syscall(0x3E2)),
    /* 63 */ ("pthread_rwlock_wrlock",StubKind::Syscall(0x3E3)),
    /* 64 */ ("pthread_rwlock_unlock",StubKind::Syscall(0x3E4)),
    /* 65 */ ("pthread_rwlock_init",  StubKind::Syscall(0x3E5)),
    /* 66 */ ("pthread_rwlock_destroy",StubKind::Syscall(0x3E6)),
    /* 67 */ ("pthread_attr_init",    StubKind::Syscall(0x3E7)),
    /* 68 */ ("pthread_attr_destroy", StubKind::Syscall(0x3E8)),
    /* 69 */ ("pthread_attr_setstacksize",StubKind::Syscall(0x3E9)),
    /* 70 */ ("pthread_attr_setdetachstate",StubKind::Syscall(0x3EA)),
    /* 71 */ ("pthread_attr_getstack",StubKind::Syscall(0x3EB)),
    /* 72 */ ("pthread_mutexattr_init",StubKind::Syscall(0x3EC)),
    /* 73 */ ("pthread_mutexattr_destroy",StubKind::Syscall(0x3ED)),
    /* 74 */ ("pthread_mutexattr_settype",StubKind::Syscall(0x3EE)),
    /* 75 */ ("pthread_condattr_init",StubKind::Syscall(0x3EF)),
    /* 76 */ ("pthread_condattr_destroy",StubKind::Syscall(0x3F0)),
    /* 77 */ ("pthread_condattr_setclock",StubKind::Syscall(0x3F1)),
    /* 78 */ ("pthread_sigmask",      StubKind::Syscall(0x3F2)),
    /* 79 */ ("pthread_kill",         StubKind::Syscall(0x3F3)),
    /* 80 */ ("pthread_setname_np",   StubKind::Syscall(0x3F4)),
    /* 81 */ ("pthread_getname_np",   StubKind::Syscall(0x3F5)),
    /* 82 */ ("pthread_getattr_np",   StubKind::Syscall(0x3F6)),
    /* 83 */ ("pthread_getschedparam",StubKind::Syscall(0x3F7)),
    // ── Semaphores ─────────────────────────────────────────────────────────
    /* 84 */ ("sem_init",             StubKind::Syscall(0x3F8)),
    /* 85 */ ("sem_destroy",          StubKind::Syscall(0x3F9)),
    /* 86 */ ("sem_wait",             StubKind::Syscall(0x3FA)),
    /* 87 */ ("sem_trywait",          StubKind::Syscall(0x3FB)),
    /* 88 */ ("sem_post",             StubKind::Syscall(0x3FC)),
    // ── TLS ────────────────────────────────────────────────────────────────
    /* 89 */ ("__tls_get_addr",       StubKind::Syscall(0x3FD)),
    // ── System ─────────────────────────────────────────────────────────────
    /* 90 */ ("abort",                StubKind::Syscall(0x400)),
    /* 91 */ ("__stack_chk_fail",     StubKind::Syscall(0x400)), // alias abort
    /* 92 */ ("exit",                 StubKind::Syscall(60)),    // linux SYS_exit
    /* 93 */ ("_exit",                StubKind::Syscall(60)),    // alias
    /* 94 */ ("getpid",               StubKind::Syscall(39)),    // linux SYS_getpid
    /* 95 */ ("getpagesize",          StubKind::RetU32(4096)),
    /* 96 */ ("sysconf",              StubKind::Syscall(0x402)),
    /* 97 */ ("nanosleep",            StubKind::Syscall(0x403)),
    /* 98 */ ("gettimeofday",         StubKind::Syscall(0x404)),
    /* 99 */ ("clock_gettime",        StubKind::Syscall(0x405)),
    /* 100 */ ("time",                StubKind::Syscall(0x406)),
    /* 101 */ ("fork",                StubKind::Syscall(0x392)), // existing
    /* 102 */ ("wait",                StubKind::Syscall(0x407)),
    /* 103 */ ("getrusage",           StubKind::Syscall(0x408)),
    /* 104 */ ("getcwd",              StubKind::Syscall(0x409)),
    /* 105 */ ("gethostname",         StubKind::Syscall(0x40A)),
    /* 106 */ ("prctl",               StubKind::Syscall(0x40B)),
    /* 107 */ ("setsid",              StubKind::Syscall(0x40C)),
    /* 108 */ ("setpriority",         StubKind::Syscall(0x40D)),
    /* 109 */ ("uname",               StubKind::Syscall(0x40E)),
    /* 110 */ ("strerror",            StubKind::Syscall(0x40F)),
    /* 111 */ ("strerror_r",          StubKind::Syscall(0x410)),
    /* 112 */ ("sched_yield",         StubKind::Syscall(0x390)), // existing
    /* 113 */ ("syscall",             StubKind::Syscall(0x411)), // passthrough
    /* 114 */ ("write",               StubKind::Syscall(1)),     // linux SYS_write
    /* 115 */ ("read",                StubKind::Syscall(0)),     // linux SYS_read
    /* 116 */ ("close",               StubKind::Syscall(3)),     // linux SYS_close
    /* 117 */ ("mmap64",              StubKind::Syscall(9)),     // linux SYS_mmap
    /* 118 */ ("munmap",              StubKind::Syscall(11)),    // linux SYS_munmap
    /* 119 */ ("mprotect",            StubKind::Syscall(10)),    // linux SYS_mprotect
    /* 120 */ ("madvise",             StubKind::Syscall(0x412)),
    /* 121 */ ("dlopen",              StubKind::Syscall(0x350)), // existing
    /* 122 */ ("dlsym",               StubKind::Syscall(0x351)), // existing
    /* 123 */ ("dlclose",             StubKind::Syscall(0x352)), // existing
    /* 124 */ ("dladdr",              StubKind::Syscall(0x413)),
    /* 125 */ ("dlerror",             StubKind::Syscall(0x414)),
    /* 126 */ ("getenv",              StubKind::Syscall(0x415)),
    /* 127 */ ("setlocale",           StubKind::Syscall(0x416)),
    /* 128 */ ("uselocale",           StubKind::Syscall(0x417)),
    /* 129 */ ("newlocale",           StubKind::Syscall(0x418)),
    /* 130 */ ("freelocale",          StubKind::Syscall(0x419)),
    /* 131 */ ("catopen",             StubKind::Syscall(0x41A)),
    /* 132 */ ("catgets",             StubKind::Syscall(0x41B)),
    /* 133 */ ("catclose",            StubKind::Syscall(0x41C)),
    // ── CType (return address of table) ────────────────────────────────────
    /* 134 */ ("__errno_location",    StubKind::RetAddr(SD_ERRNO)),
    /* 135 */ ("__ctype_b_loc",       StubKind::RetAddr(SD_CTYPE_B_PTR)),
    /* 136 */ ("__ctype_tolower_loc", StubKind::RetAddr(SD_CTYPE_LO_PTR)),
    /* 137 */ ("__ctype_toupper_loc", StubKind::RetAddr(SD_CTYPE_UP_PTR)),
    /* 138 */ ("__ctype_get_mb_cur_max", StubKind::RetU32(1)),
    /* 139 */ ("__cxa_atexit",        StubKind::Syscall(0x41D)),
    // ── IO ─────────────────────────────────────────────────────────────────
    /* 140 */ ("open64",              StubKind::Syscall(0x420)),
    /* 141 */ ("openat64",            StubKind::Syscall(257)), // Linux openat(dirfd, path, flags, mode)
    /* 141a*/ ("openat",              StubKind::Syscall(257)),
    /* 142 */ ("lseek64",             StubKind::Syscall(0x421)),
    /* 143 */ ("fopen64",             StubKind::Syscall(0x422)),
    /* 144 */ ("fclose",              StubKind::Syscall(0x423)),
    /* 145 */ ("fread",               StubKind::Syscall(0x424)),
    /* 146 */ ("fwrite",              StubKind::Syscall(0x425)),
    /* 147 */ ("fseek",               StubKind::Syscall(0x426)),
    /* 148 */ ("fseeko64",            StubKind::Syscall(0x426)), // alias
    /* 149 */ ("ftell",               StubKind::Syscall(0x427)),
    /* 150 */ ("ftello64",            StubKind::Syscall(0x427)), // alias
    /* 151 */ ("feof",                StubKind::Syscall(0x428)),
    /* 152 */ ("ferror",              StubKind::Syscall(0x429)),
    /* 153 */ ("fflush",              StubKind::Syscall(0x42A)),
    /* 154 */ ("fgets",               StubKind::Syscall(0x42B)),
    /* 155 */ ("fputc",               StubKind::Syscall(0x42C)),
    /* 156 */ ("fputs",               StubKind::Syscall(0x42D)),
    /* 157 */ ("fputwc",              StubKind::Syscall(0x42E)),
    /* 158 */ ("getwc",               StubKind::Syscall(0x42F)),
    /* 159 */ ("ungetc",              StubKind::Syscall(0x430)),
    /* 160 */ ("ungetwc",             StubKind::Syscall(0x431)),
    /* 161 */ ("fprintf",             StubKind::Syscall(0x432)),
    /* 162 */ ("vfprintf",            StubKind::Syscall(0x433)),
    /* 163 */ ("snprintf",            StubKind::Syscall(0x434)),
    /* 164 */ ("vsnprintf",           StubKind::Syscall(0x435)),
    /* 165 */ ("sprintf",             StubKind::Syscall(0x436)),
    /* 166 */ ("printf",              StubKind::Syscall(0x437)),
    /* 167 */ ("puts",                StubKind::Syscall(0x438)),
    /* 168 */ ("perror",              StubKind::Syscall(0x439)),
    /* 169 */ ("vasprintf",           StubKind::Syscall(0x43A)),
    /* 170 */ ("__isoc99_fscanf",     StubKind::Syscall(0x43B)),
    /* 171 */ ("__isoc99_sscanf",     StubKind::Syscall(0x43C)),
    /* 172 */ ("__isoc99_vsscanf",    StubKind::Syscall(0x43D)),
    /* 173 */ ("fileno",              StubKind::Syscall(0x43E)),
    /* 174 */ ("rewind",              StubKind::Syscall(0x43F)),
    /* 175 */ ("clearerr",            StubKind::Syscall(0x440)),
    /* 176 */ ("stat",                StubKind::Syscall(0x441)),
    /* 177 */ ("lstat",               StubKind::Syscall(0x441)), // alias
    /* 178 */ ("fdopen",              StubKind::Syscall(0x442)),
    /* 179 */ ("opendir",             StubKind::Syscall(0x442)),
    /* 180 */ ("closedir",            StubKind::Syscall(0x442)),
    /* 181 */ ("readdir64",           StubKind::Syscall(0x443)),
    /* 182 */ ("rewinddir",           StubKind::Syscall(0x444)),
    /* 180 */ ("setbuf",              StubKind::Syscall(0x445)),
    /* 181 */ ("setvbuf",             StubKind::Syscall(0x446)),
    /* 182 */ ("getc",                StubKind::Syscall(0x447)),
    /* 183 */ ("__getdelim",          StubKind::Syscall(0x448)),
    /* 184 */ ("pread64",             StubKind::Syscall(0x449)),
    /* 185 */ ("ftruncate64",         StubKind::Syscall(0x44A)),
    /* 186 */ ("fsync",               StubKind::Syscall(0x44B)),
    /* 187 */ ("chdir",               StubKind::Syscall(0x44C)),
    /* 188 */ ("mkdirat",             StubKind::Syscall(0x44D)),
    /* 189 */ ("renameat",            StubKind::Syscall(0x44E)),
    /* 190 */ ("unlinkat",            StubKind::Syscall(0x44F)),
    /* 191 */ ("symlinkat",           StubKind::Syscall(0x450)),
    /* 192 */ ("readlinkat",          StubKind::Syscall(0x451)),
    /* 193 */ ("readlink",            StubKind::Syscall(0x452)),
    /* 194 */ ("realpath",            StubKind::Syscall(0x453)),
    /* 195 */ ("dup",                 StubKind::Syscall(0x454)),
    /* 196 */ ("dup2",                StubKind::Syscall(0x455)),
    /* 197 */ ("pipe",                StubKind::Syscall(0x456)),
    /* 198 */ ("pipe2",               StubKind::Syscall(0x457)),
    ("fcntl",                          StubKind::Syscall(0x4A3)),
    ("fcntl64",                        StubKind::Syscall(0x4A3)),
    ("__fcntl",                        StubKind::Syscall(0x4A3)),
    /* 199 */ ("faccessat",           StubKind::Syscall(0x458)),
    /* 200 */ ("sendfile64",          StubKind::Syscall(0x459)),
    /* 201 */ ("__fxstat64",          StubKind::Syscall(0x45A)),
    /* 202 */ ("__fxstatat64",        StubKind::Syscall(0x45B)),
    /* 203 */ ("__lxstat64",          StubKind::Syscall(0x45C)),
    /* 204 */ ("__xstat64",           StubKind::Syscall(0x45D)),
    // ── Network ─────────────────────────────────────────────────────────────
    /* 205 */ ("socket",              StubKind::Syscall(0x460)),
    /* 206 */ ("bind",                StubKind::Syscall(0x461)),
    /* 207 */ ("listen",              StubKind::Syscall(0x462)),
    /* 208 */ ("connect",             StubKind::Syscall(0x463)),
    /* 209 */ ("accept4",             StubKind::Syscall(0x464)),
    /* 210 */ ("sendto",              StubKind::Syscall(0x465)),
    /* 211 */ ("sendmsg",             StubKind::Syscall(0x466)),
    /* 212 */ ("recvfrom",            StubKind::Syscall(0x467)),
    /* 213 */ ("recvmsg",             StubKind::Syscall(0x468)),
    /* 214 */ ("shutdown",            StubKind::Syscall(0x469)),
    /* 215 */ ("setsockopt",          StubKind::Syscall(0x46A)),
    /* 216 */ ("getsockopt",          StubKind::Syscall(0x46B)),
    /* 217 */ ("getsockname",         StubKind::Syscall(0x46C)),
    /* 218 */ ("getpeername",         StubKind::Syscall(0x46D)),
    /* 219 */ ("getaddrinfo",         StubKind::Syscall(0x46E)),
    /* 220 */ ("freeaddrinfo",        StubKind::Syscall(0x46F)),
    /* 221 */ ("getnameinfo",         StubKind::Syscall(0x470)),
    /* 222 */ ("gai_strerror",        StubKind::Syscall(0x471)),
    /* 223 */ ("getifaddrs",          StubKind::Syscall(0x472)),
    /* 224 */ ("freeifaddrs",         StubKind::Syscall(0x473)),
    /* 225 */ ("if_nametoindex",      StubKind::Syscall(0x474)),
    /* 226 */ ("inet_ntop",           StubKind::Syscall(0x475)),
    /* 227 */ ("inet_pton",           StubKind::Syscall(0x476)),
    /* 228 */ ("gethostname2",        StubKind::Syscall(0x477)), // dup, unused
    // ── Epoll/inotify/timer ─────────────────────────────────────────────────
    /* 229 */ ("epoll_create",        StubKind::Syscall(0x478)),
    /* 230 */ ("epoll_create1",       StubKind::Syscall(0x479)),
    /* 231 */ ("epoll_ctl",           StubKind::Syscall(0x47A)),
    /* 232 */ ("epoll_wait",          StubKind::Syscall(0x47B)),
    /* 233 */ ("inotify_init1",       StubKind::Syscall(0x47C)),
    /* 234 */ ("inotify_add_watch",   StubKind::Syscall(0x47D)),
    /* 235 */ ("inotify_rm_watch",    StubKind::Syscall(0x47E)),
    /* 236 */ ("timerfd_create",      StubKind::Syscall(0x47F)),
    /* 237 */ ("timerfd_settime",     StubKind::Syscall(0x480)),
    /* 238 */ ("poll",                StubKind::Syscall(0x481)),
    /* 239 */ ("ioctl",               StubKind::Syscall(0x482)),
    /* 240 */ ("kill",                StubKind::Syscall(62)),    // linux SYS_kill
    /* 241 */ ("sigaction",           StubKind::Syscall(0x394)), // existing
    /* 242 */ ("sigemptyset",         StubKind::Syscall(0x483)),
    /* 243 */ ("sigaddset",           StubKind::Syscall(0x484)),
    /* 244 */ ("sigfillset",          StubKind::Syscall(0x485)),
    /* 245 */ ("__setjmp_old",        StubKind::Syscall(0x486)),
    /* 246 */ ("__longjmp_old",       StubKind::Syscall(0x487)),
    // ── Locale / wchar ──────────────────────────────────────────────────────
    /* 247 */ ("wcslen",              StubKind::Syscall(0x488)),
    /* 248 */ ("mbrtowc",             StubKind::Syscall(0x489)),
    /* 249 */ ("mbsnrtowcs",          StubKind::Syscall(0x48A)),
    /* 250 */ ("mbsrtowcs",           StubKind::Syscall(0x48B)),
    /* 251 */ ("mbtowc",              StubKind::Syscall(0x48C)),
    /* 252 */ ("wcrtomb",             StubKind::Syscall(0x48D)),
    /* 253 */ ("wmemchr",             StubKind::Syscall(0x48E)),
    /* 254 */ ("strftime_l",          StubKind::Syscall(0x48F)),
    /* 255 */ ("strtod_l",            StubKind::Syscall(0x490)),
    // ── page boundary: indices 256+ → second trampoline page ──────────────
    /* 256 */ ("strtof_l",            StubKind::Syscall(0x491)),
    /* 257 */ ("strtold_l",           StubKind::Syscall(0x492)),
    /* 258 */ ("strtoll_l",           StubKind::Syscall(0x493)),
    /* 259 */ ("strtoull_l",          StubKind::Syscall(0x494)),
    /* 260 */ ("tcgetattr",           StubKind::Syscall(0x495)),
    /* 261 */ ("tcsetattr",           StubKind::Syscall(0x496)),
    /* 262 */ ("tzset",               StubKind::Syscall(0x497)),
    /* 263 */ ("localtime_r",         StubKind::Syscall(0x498)),
    /* 264 */ ("dup3",                StubKind::Syscall(0x499)),
    /* 265 */ ("execvp",              StubKind::Syscall(0x49A)),
    /* 266 */ ("syslog",              StubKind::Syscall(0x49B)),
    /* 267 */ ("ilogbf",              StubKind::Syscall(0x49C)),
    /* 268 */ ("modff",               StubKind::Syscall(0x49D)),
    /* 269 */ ("nextafterf",          StubKind::Syscall(0x49E)),
    /* 270 */ ("scalbnf",             StubKind::Syscall(0x49F)),
    /* 271 */ ("llround",             StubKind::Syscall(0x4A0)),
    /* 272 */ ("llroundf",            StubKind::Syscall(0x4A1)),
    /* 273 */ ("remainder",           StubKind::Syscall(0x4A2)),
    // ── Math (real libm via kernel syscall; result in xmm0) ─────────────────
    // Each MathSyscall* spans 2 trampoline slots, so every entry is followed by
    // a Padding slot. The syscall NRs MUST match the match arms in `libm_call`.
    ("sin",    StubKind::MathSyscall1(0x4C0)), ("__m_sin",   StubKind::Padding),
    ("sinf",   StubKind::MathSyscall1(0x4C1)), ("__m_sinf",  StubKind::Padding),
    ("cos",    StubKind::MathSyscall1(0x4C2)), ("__m_cos",   StubKind::Padding),
    ("cosf",   StubKind::MathSyscall1(0x4C3)), ("__m_cosf",  StubKind::Padding),
    ("tan",    StubKind::MathSyscall1(0x4C4)), ("__m_tan",   StubKind::Padding),
    ("tanf",   StubKind::MathSyscall1(0x4C5)), ("__m_tanf",  StubKind::Padding),
    ("asin",   StubKind::MathSyscall1(0x4C6)), ("__m_asin",  StubKind::Padding),
    ("asinf",  StubKind::MathSyscall1(0x4C7)), ("__m_asinf", StubKind::Padding),
    ("acos",   StubKind::MathSyscall1(0x4C8)), ("__m_acos",  StubKind::Padding),
    ("acosf",  StubKind::MathSyscall1(0x4C9)), ("__m_acosf", StubKind::Padding),
    ("atan",   StubKind::MathSyscall1(0x4CA)), ("__m_atan",  StubKind::Padding),
    ("atan2",  StubKind::MathSyscall2(0x4CB)), ("__m_atan2", StubKind::Padding),
    ("atan2f", StubKind::MathSyscall2(0x4CC)), ("__m_atan2f",StubKind::Padding),
    ("sinh",   StubKind::MathSyscall1(0x4CD)), ("__m_sinh",  StubKind::Padding),
    ("cosh",   StubKind::MathSyscall1(0x4CE)), ("__m_cosh",  StubKind::Padding),
    ("tanh",   StubKind::MathSyscall1(0x4CF)), ("__m_tanh",  StubKind::Padding),
    ("tanhf",  StubKind::MathSyscall1(0x4D0)), ("__m_tanhf", StubKind::Padding),
    ("exp",    StubKind::MathSyscall1(0x4D1)), ("__m_exp",   StubKind::Padding),
    ("expf",   StubKind::MathSyscall1(0x4D2)), ("__m_expf",  StubKind::Padding),
    ("exp2",   StubKind::MathSyscall1(0x4D3)), ("__m_exp2",  StubKind::Padding),
    ("exp2f",  StubKind::MathSyscall1(0x4D4)), ("__m_exp2f", StubKind::Padding),
    ("log",    StubKind::MathSyscall1(0x4D5)), ("__m_log",   StubKind::Padding),
    ("log2",   StubKind::MathSyscall1(0x4D6)), ("__m_log2",  StubKind::Padding),
    ("log2f",  StubKind::MathSyscall1(0x4D7)), ("__m_log2f", StubKind::Padding),
    ("log10",  StubKind::MathSyscall1(0x4D8)), ("__m_log10", StubKind::Padding),
    ("pow",    StubKind::MathSyscall2(0x4D9)), ("__m_pow",   StubKind::Padding),
    ("powf",   StubKind::MathSyscall2(0x4DA)), ("__m_powf",  StubKind::Padding),
    ("sqrt",   StubKind::MathSyscall1(0x4DB)), ("__m_sqrt",  StubKind::Padding),
    ("sqrtf",  StubKind::MathSyscall1(0x4DC)), ("__m_sqrtf", StubKind::Padding),
    ("cbrt",   StubKind::MathSyscall1(0x4DD)), ("__m_cbrt",  StubKind::Padding),
    ("cbrtf",  StubKind::MathSyscall1(0x4DE)), ("__m_cbrtf", StubKind::Padding),
    ("fabs",   StubKind::MathSyscall1(0x4DF)), ("__m_fabs",  StubKind::Padding),
    ("floor",  StubKind::MathSyscall1(0x4E0)), ("__m_floor", StubKind::Padding),
    ("floorf", StubKind::MathSyscall1(0x4E1)), ("__m_floorf",StubKind::Padding),
    ("ceil",   StubKind::MathSyscall1(0x4E2)), ("__m_ceil",  StubKind::Padding),
    ("ceilf",  StubKind::MathSyscall1(0x4E3)), ("__m_ceilf", StubKind::Padding),
    ("round",  StubKind::MathSyscall1(0x4E4)), ("__m_round", StubKind::Padding),
    ("roundf", StubKind::MathSyscall1(0x4E5)), ("__m_roundf",StubKind::Padding),
    ("trunc",  StubKind::MathSyscall1(0x4E6)), ("__m_trunc", StubKind::Padding),
    ("truncf", StubKind::MathSyscall1(0x4E7)), ("__m_truncf",StubKind::Padding),
    ("fmod",   StubKind::MathSyscall2(0x4E8)), ("__m_fmod",  StubKind::Padding),
    ("fmodf",  StubKind::MathSyscall2(0x4E9)), ("__m_fmodf", StubKind::Padding),
    ("fma",    StubKind::MathSyscall3(0x4EA)), ("__m_fma",   StubKind::Padding),
    ("hypot",  StubKind::MathSyscall2(0x4EB)), ("__m_hypot", StubKind::Padding),
    ("hypotf", StubKind::MathSyscall2(0x4EC)), ("__m_hypotf",StubKind::Padding),
    ("erff",   StubKind::MathSyscall1(0x4ED)), ("__m_erff",  StubKind::Padding),
    ("acosh",  StubKind::MathSyscall1(0x4EE)), ("__m_acosh", StubKind::Padding),
    ("asinh",  StubKind::MathSyscall1(0x4EF)), ("__m_asinh", StubKind::Padding),
    ("atanh",  StubKind::MathSyscall1(0x4F0)), ("__m_atanh", StubKind::Padding),
    ("log10f", StubKind::MathSyscall1(0x4F1)), ("__m_log10f",StubKind::Padding),
    // ── GNU emulated TLS (used by Flutter engine compiled without native TLS) ──
    /* 324 */ ("__emutls_get_address",       StubKind::Syscall(0x4B0)),
    /* 325 */ ("__emutls_register_common",   StubKind::Syscall(0x4B1)),
    /* 326 */ ("__emutls_get_address_compat",StubKind::Syscall(0x4B0)), // alias
    /* 327 */ ("__pthread_self",             StubKind::Syscall(0x3D1)),
    /* 328 */ ("arch_prctl",                 StubKind::Syscall(158)),
    // Internal-only: trampoline seeded at the bottom of every new thread's
    // stack so that `ret` from the thread start routine cleanly invokes
    // thread_exit with RAX (the routine's return value) as the exit code.
    // The leading "__oscortex_" name prefix guarantees no libc symbol can
    // accidentally resolve here. See THREAD_RETURN_TRAMPOLINE_VA below.
    /* 329 */ ("__oscortex_thread_return",   StubKind::SyscallRetAsArg0(0x35B)),
    /* 330 */ ("qsort",                      StubKind::Qsort),
    /* 331 */ ("__qsort_pad1",               StubKind::Padding),
    /* 332 */ ("__qsort_pad2",               StubKind::Padding),
    /* 333 */ ("__qsort_pad3",               StubKind::Padding),
    /* 334 */ ("__qsort_pad4",               StubKind::Padding),
    /* 335 */ ("__qsort_pad5",               StubKind::Padding),
    /* 336 */ ("__qsort_pad6",               StubKind::Padding),
    /* 337 */ ("__qsort_pad7",               StubKind::Padding),
    /* 338 */ ("__qsort_pad8",               StubKind::Padding),
    /* 339 */ ("_setjmp",                    StubKind::Setjmp),
    /* 340 */ ("__setjmp_pad1",              StubKind::Padding),
    /* 341 */ ("__setjmp_pad2",              StubKind::Padding),
    /* 342 */ ("longjmp",                    StubKind::Longjmp),
    /* 343 */ ("__longjmp_pad1",             StubKind::Padding),
    /* 344 */ ("__longjmp_pad2",             StubKind::Padding),
    /* 345 */ ("__oscortex_fallback_stub",   StubKind::RetU32(0)),
    /* 346 */ ("__oscortex_fallback_stub_nonnull", StubKind::RetU32(0xFC000000)),
    /* 347 */ ("__oscortex_fallback_stub_true",    StubKind::RetU32(1)),
    /* 348 */ ("__oscortex_fallback_stub_fcfontset", StubKind::RetAddr(SD_EMPTY_FC_FONTSET)),
];

/// Data symbols: resolve to a fixed VA in SYSDATA page (no trampoline needed).
const DATA_SYMS: &[(&str, u64)] = &[
    ("environ", SD_ENVIRON),
    ("stdin",   SD_STDIN),
    ("stdout",  SD_STDOUT),
    ("stderr",  SD_STDERR),
];

/// Total stub count — must not exceed 512 (two trampoline pages).
const _STUB_COUNT_CHECK: () = {
    assert!(STUBS.len() <= 512, "too many POSIX stubs — expand trampoline pages");
};

// ── Public symbol table ───────────────────────────────────────────────────────

/// (symbol_name, user_va) pairs for all glibc-compatible symbols.
/// dl.rs scans this when resolving SHN_UNDEF entries in any loaded ELF.
pub fn find_posix_symbol(name: &str) -> Option<u64> {
    // Check data symbols first.
    for &(n, va) in DATA_SYMS {
        if n == name { return Some(va); }
    }
    // Then function stubs.
    for (i, &(n, _)) in STUBS.iter().enumerate() {
        if n == name {
            return Some(TRAMPOLINE_PAGES_VA + i as u64 * 16);
        }
    }
    None
}

/// Lowest / highest libm syscall numbers (see the MathSyscall* entries above).
pub const LIBM_NR_LO: u64 = 0x4C0;
pub const LIBM_NR_HI: u64 = 0x4F1;

/// Compute a libm function for the math trampolines.
///
/// `a0`/`a1`/`a2` carry the raw IEEE-754 bit patterns of the (up to three)
/// `double` arguments — the trampoline moved `xmm0/xmm1/xmm2` into the integer
/// arg registers. `f32` variants take the low 32 bits as the float. The result
/// bit pattern is returned in `rax` and the trampoline moves it back to `xmm0`.
///
/// NRs MUST stay in sync with the `MathSyscall*` entries in `STUBS`.
pub fn libm_call(nr: u64, a0: u64, a1: u64, a2: u64) -> i64 {
    let x = f64::from_bits(a0);
    let y = f64::from_bits(a1);
    let z = f64::from_bits(a2);
    let xf = f32::from_bits(a0 as u32);
    let yf = f32::from_bits(a1 as u32);
    let bits: u64 = match nr {
        0x4C0 => libm::sin(x).to_bits(),
        0x4C1 => libm::sinf(xf).to_bits() as u64,
        0x4C2 => libm::cos(x).to_bits(),
        0x4C3 => libm::cosf(xf).to_bits() as u64,
        0x4C4 => libm::tan(x).to_bits(),
        0x4C5 => libm::tanf(xf).to_bits() as u64,
        0x4C6 => libm::asin(x).to_bits(),
        0x4C7 => libm::asinf(xf).to_bits() as u64,
        0x4C8 => libm::acos(x).to_bits(),
        0x4C9 => libm::acosf(xf).to_bits() as u64,
        0x4CA => libm::atan(x).to_bits(),
        0x4CB => libm::atan2(x, y).to_bits(),
        0x4CC => libm::atan2f(xf, yf).to_bits() as u64,
        0x4CD => libm::sinh(x).to_bits(),
        0x4CE => libm::cosh(x).to_bits(),
        0x4CF => libm::tanh(x).to_bits(),
        0x4D0 => libm::tanhf(xf).to_bits() as u64,
        0x4D1 => libm::exp(x).to_bits(),
        0x4D2 => libm::expf(xf).to_bits() as u64,
        0x4D3 => libm::exp2(x).to_bits(),
        0x4D4 => libm::exp2f(xf).to_bits() as u64,
        0x4D5 => libm::log(x).to_bits(),
        0x4D6 => libm::log2(x).to_bits(),
        0x4D7 => libm::log2f(xf).to_bits() as u64,
        0x4D8 => libm::log10(x).to_bits(),
        0x4D9 => libm::pow(x, y).to_bits(),
        0x4DA => libm::powf(xf, yf).to_bits() as u64,
        0x4DB => libm::sqrt(x).to_bits(),
        0x4DC => libm::sqrtf(xf).to_bits() as u64,
        0x4DD => libm::cbrt(x).to_bits(),
        0x4DE => libm::cbrtf(xf).to_bits() as u64,
        0x4DF => libm::fabs(x).to_bits(),
        0x4E0 => libm::floor(x).to_bits(),
        0x4E1 => libm::floorf(xf).to_bits() as u64,
        0x4E2 => libm::ceil(x).to_bits(),
        0x4E3 => libm::ceilf(xf).to_bits() as u64,
        0x4E4 => libm::round(x).to_bits(),
        0x4E5 => libm::roundf(xf).to_bits() as u64,
        0x4E6 => libm::trunc(x).to_bits(),
        0x4E7 => libm::truncf(xf).to_bits() as u64,
        0x4E8 => libm::fmod(x, y).to_bits(),
        0x4E9 => libm::fmodf(xf, yf).to_bits() as u64,
        0x4EA => libm::fma(x, y, z).to_bits(),
        0x4EB => libm::hypot(x, y).to_bits(),
        0x4EC => libm::hypotf(xf, yf).to_bits() as u64,
        0x4ED => libm::erff(xf).to_bits() as u64,
        0x4EE => libm::acosh(x).to_bits(),
        0x4EF => libm::asinh(x).to_bits(),
        0x4F0 => libm::atanh(x).to_bits(),
        0x4F1 => libm::log10f(xf).to_bits() as u64,
        _ => 0,
    };
    bits as i64
}

// ── Ctype table generator ─────────────────────────────────────────────────────

/// Generate the POSIX C-locale ctype_b table (384 u16 entries centered at 128).
/// Entry at index 128+c gives classification bits for unsigned char c.
fn gen_ctype_b() -> [u16; 384] {
    const ISupper: u16  = 1 << 0;
    const ISlower: u16  = 1 << 1;
    const ISalpha: u16  = 1 << 2;
    const ISdigit: u16  = 1 << 3;
    const ISxdigit: u16 = 1 << 4;
    const ISspace: u16  = 1 << 5;
    const ISprint: u16  = 1 << 6;
    const ISgraph: u16  = 1 << 7;
    const ISblank: u16  = 1 << 8;
    const IScntrl: u16  = 1 << 9;
    const ISpunct: u16  = 1 << 10;
    const ISalnum: u16  = 1 << 11;

    let mut t = [0u16; 384];
    let b = 128usize; // base offset

    // Control characters 0-31 and 127
    for i in 0u16..32 { t[b + i as usize] |= IScntrl; }
    t[b + 127] |= IScntrl;

    // TAB (9): space + blank + cntrl
    t[b + 9] |= ISspace | ISblank;
    // NL VT FF CR (10-13): space
    for i in 10u16..14 { t[b + i as usize] |= ISspace; }
    // Space (32): space + blank + print
    t[b + 32] |= ISspace | ISblank | ISprint;

    // Printable 33-126
    for i in 33u16..127 { t[b + i as usize] |= ISprint | ISgraph; }

    // Digits 48-57
    for i in 48u16..58 {
        t[b + i as usize] |= ISdigit | ISxdigit | ISalnum;
    }
    // Hex A-F 65-70
    for i in 65u16..71 { t[b + i as usize] |= ISxdigit; }
    // Hex a-f 97-102
    for i in 97u16..103 { t[b + i as usize] |= ISxdigit; }
    // Upper A-Z 65-90
    for i in 65u16..91 { t[b + i as usize] |= ISupper | ISalpha | ISalnum; }
    // Lower a-z 97-122
    for i in 97u16..123 { t[b + i as usize] |= ISlower | ISalpha | ISalnum; }
    // Punct: printable non-alnum non-space
    for i in 33u16..127 {
        if t[b + i as usize] & (ISalnum | ISspace) == 0 {
            t[b + i as usize] |= ISpunct;
        }
    }
    t
}

/// tolower table: identity except A-Z → a-z.
fn gen_ctype_lo() -> [i32; 384] {
    let mut t = [0i32; 384];
    let b = 128usize;
    for i in 0u32..256 {
        let c = i as i32;
        t[b + i as usize] = if i >= 65 && i <= 90 { c + 32 } else { c };
    }
    t
}

/// toupper table: identity except a-z → A-Z.
fn gen_ctype_up() -> [i32; 384] {
    let mut t = [0i32; 384];
    let b = 128usize;
    for i in 0u32..256 {
        let c = i as i32;
        t[b + i as usize] = if i >= 97 && i <= 122 { c - 32 } else { c };
    }
    t
}

// ── Page mapping ──────────────────────────────────────────────────────────────

fn write_u64_le(buf: &mut [u8], off: usize, v: u64) {
    buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

/// Map the POSIX system pages into a user address space.
///
/// Call this from `process::spawn` right after the stack is mapped and before
/// the process is added to the process table.
pub fn map_system_pages(pml4_phys: u64) -> Result<(), &'static str> {
    map_trampoline_pages(pml4_phys)?;
    map_sysdata_page(pml4_phys)?;
    Ok(())
}

fn map_trampoline_pages(pml4_phys: u64) -> Result<(), &'static str> {
    // Allocate 2 physical frames for the trampoline pages.
    let phys0 = frame_allocator::alloc_frame().ok_or("OOM: trampoline page 0")?;
    let phys1 = frame_allocator::alloc_frame().ok_or("OOM: trampoline page 1")?;

    let hhdm = frame_allocator::hhdm_offset();
    let page0 = unsafe {
        core::slice::from_raw_parts_mut((phys0 + hhdm) as *mut u8, 4096)
    };
    let page1 = unsafe {
        core::slice::from_raw_parts_mut((phys1 + hhdm) as *mut u8, 4096)
    };
    page0.fill(0x90); // nop
    page1.fill(0x90); // nop

    for (i, &(_, kind)) in STUBS.iter().enumerate() {
        let page = if i < 256 { &mut *page0 } else { &mut *page1 };
        let off = (i % 256) * 16;
        encode_stub(page, off, kind);
    }

    // Map as read-execute (not writable) for user.
    unsafe {
        paging::map_user_page_with_flags(pml4_phys, TRAMPOLINE_PAGES_VA,       phys0, false, true)?;
        paging::map_user_page_with_flags(pml4_phys, TRAMPOLINE_PAGES_VA + 0x1000, phys1, false, true)?;
    }
    Ok(())
}

fn encode_stub(page: &mut [u8], off: usize, kind: StubKind) {
    match kind {
        StubKind::Syscall(nr) => {
            // 49 89 CA           mov r10, rcx
            // B8 xx xx xx xx     mov eax, imm32
            // 0F 05              syscall
            // C3                 ret
            // 90 * 5             nop padding
            page[off]     = 0x49;
            page[off + 1] = 0x89;
            page[off + 2] = 0xCA;
            page[off + 3] = 0xB8;
            page[off + 4..off + 8].copy_from_slice(&nr.to_le_bytes());
            page[off + 8]  = 0x0F;
            page[off + 9]  = 0x05;
            page[off + 10] = 0xC3;
            // bytes 11-15 already 0x90
        }
        StubKind::FloatZero => {
            // 0F 57 C0    xorps xmm0, xmm0
            // C3          ret
            // 90 * 12     nop padding
            page[off]     = 0x0F;
            page[off + 1] = 0x57;
            page[off + 2] = 0xC0;
            page[off + 3] = 0xC3;
            // bytes 4-15 already 0x90
        }
        StubKind::RetAddr(va) => {
            // 48 B8 xx xx xx xx xx xx xx xx    movabs rax, imm64
            // C3                                ret
            // 90 * 5                            nop padding
            page[off]     = 0x48;
            page[off + 1] = 0xB8;
            page[off + 2..off + 10].copy_from_slice(&va.to_le_bytes());
            page[off + 10] = 0xC3;
            // bytes 11-15 already 0x90
        }
        StubKind::RetU32(v) => {
            // B8 xx xx xx xx    mov eax, imm32
            // C3                ret
            // 90 * 10           nop padding
            page[off]     = 0xB8;
            page[off + 1..off + 5].copy_from_slice(&v.to_le_bytes());
            page[off + 5] = 0xC3;
            // bytes 6-15 already 0x90
        }
        StubKind::SyscallRetAsArg0(nr) => {
            // 48 89 C7           mov rdi, rax        (return value -> arg0)
            // B8 xx xx xx xx     mov eax, imm32      (syscall nr)
            // 0F 05              syscall
            // 0F 0B              ud2                 (noreturn — trap if it returns)
            // 90 * 5             nop padding
            page[off]     = 0x48;
            page[off + 1] = 0x89;
            page[off + 2] = 0xC7;
            page[off + 3] = 0xB8;
            page[off + 4..off + 8].copy_from_slice(&nr.to_le_bytes());
            page[off + 8]  = 0x0F;
            page[off + 9]  = 0x05;
            page[off + 10] = 0x0F;
            page[off + 11] = 0x0B;
            // bytes 12-15 already 0x90
        }
        StubKind::Qsort => {
            let code: &[u8] = &[
                0x53, 0x55, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x49, 0x89, 0xfc, 0x49, 0x89, 0xf5,
                0x49, 0x89, 0xd6, 0x49, 0x89, 0xcf, 0x49, 0x83, 0xfd, 0x02, 0x72, 0x61, 0x48, 0xc7, 0xc3, 0x01,
                0x00, 0x00, 0x00, 0x4c, 0x39, 0xeb, 0x73, 0x55, 0x48, 0x89, 0xdd, 0x48, 0x85, 0xed, 0x74, 0x48,
                0x48, 0x89, 0xe8, 0x49, 0x0f, 0xaf, 0xc6, 0x4c, 0x01, 0xe0, 0x48, 0x89, 0xc7, 0x4c, 0x29, 0xf0,
                0x48, 0x89, 0xc6, 0x41, 0xff, 0xd7, 0x85, 0xc0, 0x7d, 0x2e, 0x48, 0x89, 0xef, 0x49, 0x0f, 0xaf,
                0xfe, 0x4c, 0x01, 0xe7, 0x48, 0x89, 0xfe, 0x4c, 0x29, 0xf6, 0x48, 0x31, 0xc9, 0x4c, 0x39, 0xf1,
                0x73, 0x11, 0x8a, 0x04, 0x0f, 0x8a, 0x14, 0x0e, 0x88, 0x04, 0x0e, 0x88, 0x14, 0x0f, 0x48, 0xff,
                0xc1, 0xeb, 0xea, 0x48, 0xff, 0xcd, 0xeb, 0xb3, 0x48, 0xff, 0xc3, 0xeb, 0xa6, 0x41, 0x5f, 0x41,
                0x5e, 0x41, 0x5d, 0x41, 0x5c, 0x5d, 0x5b, 0xc3
            ];
            page[off..off + code.len()].copy_from_slice(code);
        }
        StubKind::MathSyscall1(nr) => {
            // movq rdi, xmm0 ; mov eax, nr ; syscall ; movq xmm0, rax ; ret
            let code: &[u8] = &[
                0x66, 0x48, 0x0F, 0x7E, 0xC7,                 // movq rdi, xmm0
                0xB8, (nr & 0xFF) as u8, ((nr >> 8) & 0xFF) as u8,
                      ((nr >> 16) & 0xFF) as u8, ((nr >> 24) & 0xFF) as u8, // mov eax, nr
                0x0F, 0x05,                                   // syscall
                0x66, 0x48, 0x0F, 0x6E, 0xC0,                 // movq xmm0, rax
                0xC3,                                         // ret
            ];
            page[off..off + code.len()].copy_from_slice(code);
        }
        StubKind::MathSyscall2(nr) => {
            // movq rdi,xmm0 ; movq rsi,xmm1 ; mov eax,nr ; syscall ; movq xmm0,rax ; ret
            let code: &[u8] = &[
                0x66, 0x48, 0x0F, 0x7E, 0xC7,                 // movq rdi, xmm0
                0x66, 0x48, 0x0F, 0x7E, 0xCE,                 // movq rsi, xmm1
                0xB8, (nr & 0xFF) as u8, ((nr >> 8) & 0xFF) as u8,
                      ((nr >> 16) & 0xFF) as u8, ((nr >> 24) & 0xFF) as u8, // mov eax, nr
                0x0F, 0x05,                                   // syscall
                0x66, 0x48, 0x0F, 0x6E, 0xC0,                 // movq xmm0, rax
                0xC3,                                         // ret
            ];
            page[off..off + code.len()].copy_from_slice(code);
        }
        StubKind::MathSyscall3(nr) => {
            // movq rdi,xmm0 ; movq rsi,xmm1 ; movq rdx,xmm2 ; mov eax,nr ; syscall ; movq xmm0,rax ; ret
            let code: &[u8] = &[
                0x66, 0x48, 0x0F, 0x7E, 0xC7,                 // movq rdi, xmm0
                0x66, 0x48, 0x0F, 0x7E, 0xCE,                 // movq rsi, xmm1
                0x66, 0x48, 0x0F, 0x7E, 0xD2,                 // movq rdx, xmm2
                0xB8, (nr & 0xFF) as u8, ((nr >> 8) & 0xFF) as u8,
                      ((nr >> 16) & 0xFF) as u8, ((nr >> 24) & 0xFF) as u8, // mov eax, nr
                0x0F, 0x05,                                   // syscall
                0x66, 0x48, 0x0F, 0x6E, 0xC0,                 // movq xmm0, rax
                0xC3,                                         // ret
            ];
            page[off..off + code.len()].copy_from_slice(code);
        }
        StubKind::Padding => {}
        StubKind::Setjmp => {
            let code: &[u8] = &[
                0x48, 0x89, 0x1f, 0x48, 0x8d, 0x44, 0x24, 0x08, 0x48, 0x89, 0x47, 0x08, 0x48, 0x89, 0x6f, 0x10,
                0x4c, 0x89, 0x67, 0x18, 0x4c, 0x89, 0x6f, 0x20, 0x4c, 0x89, 0x77, 0x28, 0x4c, 0x89, 0x7f, 0x30,
                0x48, 0x8b, 0x04, 0x24, 0x48, 0x89, 0x47, 0x38, 0x48, 0x31, 0xc0, 0xc3
            ];
            page[off..off + code.len()].copy_from_slice(code);
        }
        StubKind::Longjmp => {
            let code: &[u8] = &[
                0x48, 0x8b, 0x1f, 0x48, 0x8b, 0x67, 0x08, 0x48, 0x8b, 0x6f, 0x10, 0x4c, 0x8b, 0x67, 0x18, 0x4c,
                0x8b, 0x6f, 0x20, 0x4c, 0x8b, 0x77, 0x28, 0x4c, 0x8b, 0x7f, 0x30, 0x48, 0x8b, 0x57, 0x38, 0x48,
                0x89, 0xf0, 0x48, 0x85, 0xc0, 0x75, 0x07, 0x48, 0xc7, 0xc0, 0x01, 0x00, 0x00, 0x00, 0xff, 0xe2
            ];
            page[off..off + code.len()].copy_from_slice(code);
        }
    }
}

fn map_sysdata_page(pml4_phys: u64) -> Result<(), &'static str> {
    let phys = frame_allocator::alloc_frame().ok_or("OOM: sysdata page")?;
    let hhdm = frame_allocator::hhdm_offset();
    let page = unsafe {
        core::slice::from_raw_parts_mut((phys + hhdm) as *mut u8, 4096)
    };
    page.fill(0);

    // Write ctype pointer vars (absolute VAs of the table mid-points).
    write_u64_le(page, 0,  CTYPE_B_PTR_VAL);   // SD_CTYPE_B_PTR
    write_u64_le(page, 8,  CTYPE_LO_PTR_VAL);  // SD_CTYPE_LO_PTR
    write_u64_le(page, 16, CTYPE_UP_PTR_VAL);  // SD_CTYPE_UP_PTR
    // SD_ERRNO at offset 32: already 0
    // SD_ENVIRON pointing to SYSDATA_VA + 24 (which contains 0 = empty environment array)
    write_u64_le(page, 48, SYSDATA_VA + 24);
    // SD_STDIN/STDOUT/STDERR pointing to 0, 1, 2 to match file handles
    write_u64_le(page, 56, 0);
    write_u64_le(page, 64, 1);
    write_u64_le(page, 72, 2);

    // Write ctype_b table at offset 80.
    let ctype_b = gen_ctype_b();
    let b_off = 80usize;
    for (i, &v) in ctype_b.iter().enumerate() {
        let o = b_off + i * 2;
        page[o]     = (v & 0xFF) as u8;
        page[o + 1] = (v >> 8) as u8;
    }

    // Write ctype_lo table at offset 848.
    let ctype_lo = gen_ctype_lo();
    let lo_off = 848usize;
    for (i, &v) in ctype_lo.iter().enumerate() {
        let o = lo_off + i * 4;
        let vb = v.to_le_bytes();
        page[o..o + 4].copy_from_slice(&vb);
    }

    // Write ctype_up table at offset 2384.
    let ctype_up = gen_ctype_up();
    let up_off = 2384usize;
    for (i, &v) in ctype_up.iter().enumerate() {
        let o = up_off + i * 4;
        let vb = v.to_le_bytes();
        page[o..o + 4].copy_from_slice(&vb);
    }

    // Write stable "C" locale string at offset 3920.
    page[3920] = b'C';
    page[3921] = 0;

    // Write static library names for dladdr at offset 3922
    let path_flutter = b"/system/lib/libflutter_engine.so\0";
    page[3922..3922 + path_flutter.len()].copy_from_slice(path_flutter);

    let path_app = b"/system/flutter/libapp.so\0";
    page[3960..3960 + path_app.len()].copy_from_slice(path_app);

    let path_fallback = b"/system/lib/unknown.so\0";
    page[3990..3990 + path_fallback.len()].copy_from_slice(path_fallback);

    // Write empty FcFontSet at offset 4032.
    // nfont = 0 (offset 4032), sfont = 0 (offset 4036), fonts = NULL (offset 4040)
    page[4032..4036].copy_from_slice(&0u32.to_le_bytes());
    page[4036..4040].copy_from_slice(&0u32.to_le_bytes());
    page[4040..4048].copy_from_slice(&0u64.to_le_bytes());

    // Map as read-write (data page, not executable). errno and other mutable
    // fields (SD_ERRNO etc.) are written directly by user code via pointers.
    unsafe {
        paging::map_user_page_with_flags(pml4_phys, SYSDATA_VA, phys, true, false)?;
    }
    Ok(())
}

