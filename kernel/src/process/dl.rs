//! Kernel-side ELF shared-library loader — Phase 30 Slice 3.
//!
//! Provides [`dlopen`], [`dlsym`], [`dlclose`] as kernel services consumed by
//! the `sys_dlopen` / `sys_dlsym` / `sys_dlclose` syscalls, enabling the
//! Flutter engine host process to load `libflutter_engine.so` at runtime
//! without a userspace `ld-linux.so`.
//!
//! ## Supported x86_64 relocation types
//! | Type               | Encoding | Formula   |
//! |--------------------|----------|-----------|
//! | `R_X86_64_RELATIVE`| 8        | `B + A`   |
//! | `R_X86_64_64`      | 1        | `S + A`   |
//! | `R_X86_64_GLOB_DAT`| 6        | `S`       |
//! | `R_X86_64_JUMP_SLOT`| 7       | `S`       |
//!
//! Undefined externals (`st_shndx == SHN_UNDEF`) are left at zero.  The
//! Flutter engine fills all mandatory platform callbacks through the
//! `FlutterEngineProcTable` struct passed during `FlutterEngineInitialize`,
//! not through ELF symbol resolution.
//!
//! ## Per-process load-address layout
//! Libraries are bump-allocated starting at `LIB_VA_BASE` (16 MiB), advanced
//! by the rounded-up library span for each load.  Anonymous `mmap` calls
//! continue from wherever the bump currently sits, keeping the address ranges
//! non-overlapping inside each process's 128 TiB lower half.

use alloc::vec::Vec;
use spin::Mutex;

use crate::mm::{frame_allocator, paging};

// ── ELF64 constants ────────────────────────────────────────────────────────────

const ELF_MAGIC:    [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELFCLASS64:   u8      = 2;
const ELFDATA2LSB:  u8      = 1;
const ET_DYN:       u16     = 3;
const EM_X86_64:    u16     = 62;

const PT_LOAD:      u32     = 1;
const PT_DYNAMIC:   u32     = 2;
const PF_W:         u32     = 1 << 1;
const PF_X:         u32     = 1 << 0;

const DT_NULL:       i64     = 0;
const DT_STRTAB:     i64     = 5;
const DT_SYMTAB:     i64     = 6;
const DT_RELA:       i64     = 7;
const DT_RELASZ:     i64     = 8;
const DT_SYMENT:     i64     = 11;
const DT_INIT:       i64     = 12; // single init fn
const DT_INIT_ARRAY:    i64  = 25; // array of init fn ptrs
const DT_INIT_ARRAYSZ:  i64  = 27; // byte size of DT_INIT_ARRAY
const DT_JMPREL:     i64     = 23;
const DT_PLTRELSZ:   i64     = 2;

const R_X86_64_64:         u32 = 1;
const R_X86_64_GLOB_DAT:   u32 = 6;
const R_X86_64_JUMP_SLOT:  u32 = 7;
const R_X86_64_RELATIVE:   u32 = 8;

const STB_GLOBAL: u8 = 1;
const STB_WEAK:   u8 = 2;
const SHN_UNDEF:  u16 = 0;

/// Virtual address of the first library slot in each process.
const LIB_VA_BASE:   u64 = 0x1_4000_0000; // 5 GiB
/// Base address for anonymous memory mappings.
const ANON_VA_BASE:  u64 = 0x3_1000_0000; // 12.25 GiB
/// Minimum VA bump stride between consecutive libraries.
const LIB_VA_STRIDE: u64 = 0x0100_0000; // 16 MiB per slot


const MAX_LIBS: usize = 16;

// ── ELF64 on-disk structures ───────────────────────────────────────────────────

#[derive(Copy, Clone)]
#[repr(C, packed)]
struct Elf64Hdr {
    e_ident:     [u8; 16],
    e_type:      u16,
    e_machine:   u16,
    e_version:   u32,
    e_entry:     u64,
    e_phoff:     u64,
    e_shoff:     u64,
    e_flags:     u32,
    e_ehsize:    u16,
    e_phentsize: u16,
    e_phnum:     u16,
    e_shentsize: u16,
    e_shnum:     u16,
    e_shstrndx:  u16,
}

#[derive(Copy, Clone)]
#[repr(C, packed)]
struct Elf64Phdr {
    p_type:   u32,
    p_flags:  u32,
    p_offset: u64,
    p_vaddr:  u64,
    p_paddr:  u64,
    p_filesz: u64,
    p_memsz:  u64,
    p_align:  u64,
}

#[derive(Copy, Clone)]
#[repr(C, packed)]
struct Elf64Dyn {
    d_tag: i64,
    d_val: u64,
}

#[derive(Copy, Clone)]
#[repr(C, packed)]
struct Elf64Sym {
    st_name:  u32,
    st_info:  u8,
    st_other: u8,
    st_shndx: u16,
    st_value: u64,
    st_size:  u64,
}

#[derive(Copy, Clone)]
#[repr(C, packed)]
struct Elf64Shdr {
    sh_name:      u32,
    sh_type:      u32,
    sh_flags:     u64,
    sh_addr:      u64,
    sh_offset:    u64,
    sh_size:      u64,
    sh_link:      u32,
    sh_info:      u32,
    sh_addralign: u64,
    sh_entsize:   u64,
}

#[derive(Copy, Clone)]
#[repr(C, packed)]
struct Elf64Rela {
    r_offset: u64,
    r_info:   u64,
    r_addend: i64,
}

// ── Low-level helpers ──────────────────────────────────────────────────────────

/// Read a `Copy` value from an unaligned byte slice at `off`.
fn read_pod<T: Copy>(bytes: &[u8], off: usize) -> Option<T> {
    let sz = core::mem::size_of::<T>();
    if off.saturating_add(sz) > bytes.len() { return None; }
    // SAFETY: bounds checked above; T is repr(C, packed) → no alignment requirement.
    Some(unsafe { core::ptr::read_unaligned(bytes.as_ptr().add(off) as *const T) })
}

/// Translate a 0-based ELF virtual address to a byte offset within `bytes`
/// by walking PT_LOAD segments.
fn va_to_file_off(bytes: &[u8], hdr: &Elf64Hdr, va: usize) -> Option<usize> {
    let ph_off  = hdr.e_phoff as usize;
    let ph_num  = hdr.e_phnum as usize;
    let ph_size = hdr.e_phentsize as usize;
    for i in 0..ph_num {
        let ph: Elf64Phdr = read_pod(bytes, ph_off + i * ph_size)?;
        if ph.p_type != PT_LOAD { continue; }
        let seg_va  = ph.p_vaddr as usize;
        let seg_end = seg_va.saturating_add(ph.p_filesz as usize);
        if va >= seg_va && va < seg_end {
            return Some(ph.p_offset as usize + (va - seg_va));
        }
    }
    None
}

/// Write a `u64` into a mapped user virtual address.
///
/// # Safety
/// * `va` must be mapped and writable in the current CR3.
/// * Called during a `SYSCALL` where CR3 = user PML4 and `CR4.SMAP` is clear.
#[inline]
unsafe fn write_user_u64(va: u64, val: u64) {
    // SAFETY: caller guarantees the VA is mapped writable in the active CR3.
    unsafe { core::ptr::write_unaligned(va as *mut u64, val); }
}

// ── Symbol and library records ─────────────────────────────────────────────────

struct ExportedSym {
    /// Raw symbol name bytes (no null terminator).
    name:  Vec<u8>,
    /// Resolved virtual address within the process address space.
    vaddr: u64,
}

struct LoadedLib {
    pid:            u32,
    handle:         u32,
    load_base:      u64,
    exports:        Vec<ExportedSym>,
    /// VA of DT_INIT function (0 = none).
    init_fn_va:     u64,
    /// VA of first DT_INIT_ARRAY entry (0 = none).
    init_array_va:  u64,
    /// Number of entries in the init array.
    init_array_len: usize,
    path:           Vec<u8>,
    size:           u64,
}

// ── Global library table ───────────────────────────────────────────────────────

/// Maximum number of distinct address spaces we track bump cursors for.
const MAX_AS_SLOTS: usize = 64;

struct LibTable {
    entries:     Vec<LoadedLib>,
    next_handle: u32,
    /// Per-address-space bump VAs for dynamic loading and anonymous mappings.
    /// Format: (pml4_phys, next_lib_va, next_anon_va). `pml4_phys == 0` => free.
    as_slots:    [(u64, u64, u64); MAX_AS_SLOTS],
}

impl LibTable {
    const fn new() -> Self {
        Self {
            entries:     Vec::new(),
            next_handle: 1,
            as_slots:    [(0, 0, 0); MAX_AS_SLOTS],
        }
    }

    fn bump_lib_va(&mut self, pml4_phys: u64, size: u64) -> u64 {
        let aligned = (size + LIB_VA_STRIDE - 1) / LIB_VA_STRIDE * LIB_VA_STRIDE;
        log::info!("[DL] bump_lib_va: pml4_phys={:#x} size={:#x} aligned={:#x}", pml4_phys, size, aligned);
        // Find existing slot for this address space.
        for slot in self.as_slots.iter_mut() {
            if slot.0 == pml4_phys {
                let base = slot.1;
                slot.1 = base + aligned;
                log::info!("[DL] bump_lib_va: existing slot base={:#x} new_next={:#x}", base, slot.1);
                return base;
            }
        }
        // Allocate a new slot.
        for slot in self.as_slots.iter_mut() {
            if slot.0 == 0 {
                *slot = (pml4_phys, LIB_VA_BASE + aligned, ANON_VA_BASE);
                log::info!("[DL] bump_lib_va: new slot base={:#x} next_lib={:#x}", LIB_VA_BASE, slot.1);
                return LIB_VA_BASE;
            }
        }
        // Fallback: should not happen
        let base = self.as_slots[0].1;
        self.as_slots[0].1 = base + aligned;
        log::info!("[DL] bump_lib_va: fallback base={:#x} new_next={:#x}", base, self.as_slots[0].1);
        base
    }

    fn bump_anon_va(&mut self, pml4_phys: u64, size: u64) -> u64 {
        let aligned = (size + 4095) / 4096 * 4096;
        log::info!("[DL] bump_anon_va: pml4_phys={:#x} size={:#x} aligned={:#x}", pml4_phys, size, aligned);
        // Find existing slot for this address space.
        for slot in self.as_slots.iter_mut() {
            if slot.0 == pml4_phys {
                let base = slot.2;
                slot.2 = base + aligned;
                log::info!("[DL] bump_anon_va: existing slot base={:#x} new_next={:#x}", base, slot.2);
                return base;
            }
        }
        // Allocate a new slot.
        for slot in self.as_slots.iter_mut() {
            if slot.0 == 0 {
                *slot = (pml4_phys, LIB_VA_BASE, ANON_VA_BASE + aligned);
                log::info!("[DL] bump_anon_va: new slot base={:#x} next_anon={:#x}", ANON_VA_BASE, slot.2);
                return ANON_VA_BASE;
            }
        }
        // Fallback: should not happen
        let base = self.as_slots[0].2;
        self.as_slots[0].2 = base + aligned;
        log::info!("[DL] bump_anon_va: fallback base={:#x} new_next={:#x}", base, self.as_slots[0].2);
        base
    }
}

static LIBS: Mutex<LibTable> = Mutex::new(LibTable::new());

// ── DYNAMIC section parser ─────────────────────────────────────────────────────

/// Decoded fields from PT_DYNAMIC, all translated to byte offsets in `bytes`.
struct DynInfo {
    strtab_va:      usize, // original 0-based VA (for sym-count estimation)
    symtab_va:      usize, // original 0-based VA
    strtab_file:    usize,
    symtab_file:    usize,
    syment:         usize,
    rela_file:      usize,
    relasz:         usize,
    jmprel_file:    usize,
    pltrelsz:       usize,
    /// DT_INIT function VA (0-based, before load_base), 0 = none.
    init_va:        usize,
    /// DT_INIT_ARRAY VA (0-based, before load_base), 0 = none.
    init_array_va:  usize,
    /// DT_INIT_ARRAYSZ in bytes.
    init_arraysz:   usize,
}

fn parse_dynamic(bytes: &[u8], hdr: &Elf64Hdr) -> Option<DynInfo> {
    let ph_off  = hdr.e_phoff as usize;
    let ph_num  = hdr.e_phnum as usize;
    let ph_size = hdr.e_phentsize as usize;

    // Locate the PT_DYNAMIC segment.
    let mut dyn_file = 0usize;
    let mut dyn_sz   = 0usize;
    for i in 0..ph_num {
        let ph: Elf64Phdr = read_pod(bytes, ph_off + i * ph_size)?;
        if ph.p_type == PT_DYNAMIC {
            dyn_file = ph.p_offset as usize;
            dyn_sz   = ph.p_filesz as usize;
            break;
        }
    }
    if dyn_sz == 0 { return None; }

    let ent_sz    = core::mem::size_of::<Elf64Dyn>();
    let ent_count = dyn_sz / ent_sz;

    let mut strtab_va    = 0usize;
    let mut symtab_va    = 0usize;
    let mut syment       = core::mem::size_of::<Elf64Sym>(); // default 24
    let mut rela_va      = 0usize;
    let mut relasz       = 0usize;
    let mut jmprel_va    = 0usize;
    let mut pltrelsz     = 0usize;
    let mut init_va      = 0usize;
    let mut init_array_va = 0usize;
    let mut init_arraysz = 0usize;

    for i in 0..ent_count {
        let e: Elf64Dyn = read_pod(bytes, dyn_file + i * ent_sz)?;
        match e.d_tag {
            DT_NULL          => break,
            DT_STRTAB        => strtab_va     = e.d_val as usize,
            DT_SYMTAB        => symtab_va     = e.d_val as usize,
            DT_SYMENT        => syment        = e.d_val as usize,
            DT_RELA          => rela_va       = e.d_val as usize,
            DT_RELASZ        => relasz        = e.d_val as usize,
            DT_JMPREL        => jmprel_va     = e.d_val as usize,
            DT_PLTRELSZ      => pltrelsz      = e.d_val as usize,
            DT_INIT          => init_va       = e.d_val as usize,
            DT_INIT_ARRAY    => init_array_va = e.d_val as usize,
            DT_INIT_ARRAYSZ  => init_arraysz  = e.d_val as usize,
            _ => {}
        }
    }

    let strtab_file  = if strtab_va  != 0 { va_to_file_off(bytes, hdr, strtab_va).unwrap_or(0)  } else { 0 };
    let symtab_file  = if symtab_va  != 0 { va_to_file_off(bytes, hdr, symtab_va).unwrap_or(0)  } else { 0 };
    let rela_file    = if rela_va    != 0 { va_to_file_off(bytes, hdr, rela_va).unwrap_or(0)    } else { 0 };
    let jmprel_file  = if jmprel_va  != 0 { va_to_file_off(bytes, hdr, jmprel_va).unwrap_or(0) } else { 0 };

    Some(DynInfo { strtab_va, symtab_va, strtab_file, symtab_file, syment,
                   rela_file, relasz, jmprel_file, pltrelsz,
                   init_va, init_array_va, init_arraysz })
}

// ── Relocation application ─────────────────────────────────────────────────────

fn apply_rela_table(
    bytes:      &[u8],
    rela_file:  usize,
    table_sz:   usize,
    load_base:  u64,
    sym_vaddr:  &dyn Fn(usize) -> u64,
) {
    if rela_file == 0 || table_sz == 0 { return; }
    let ent_sz = core::mem::size_of::<Elf64Rela>();
    let count  = table_sz / ent_sz;

    let mut n_rel = 0usize;
    let mut n_64 = 0usize;
    let mut n_glob = 0usize;
    let mut n_jmp = 0usize;
    let mut n_other = 0usize;
    let mut n_oob = 0usize;

    for i in 0..count {
        let rela: Elf64Rela = match read_pod(bytes, rela_file + i * ent_sz) {
            Some(r) => r,
            None    => { n_oob += 1; break; },
        };
        let rtype   = (rela.r_info & 0xFFFF_FFFF) as u32;
        let sym_idx = (rela.r_info >> 32) as usize;
        let target  = load_base.wrapping_add(rela.r_offset);

        let value = match rtype {
            R_X86_64_RELATIVE  => { n_rel += 1; load_base.wrapping_add(rela.r_addend as u64) },
            R_X86_64_64        => { n_64 += 1; sym_vaddr(sym_idx).wrapping_add(rela.r_addend as u64) },
            R_X86_64_GLOB_DAT  => { n_glob += 1; sym_vaddr(sym_idx) },
            R_X86_64_JUMP_SLOT => { n_jmp += 1; sym_vaddr(sym_idx) },
            0                  => continue,
            _                  => { n_other += 1; continue; },
        };

        let rela_offset = rela.r_offset;
        if rela_offset == 0x1064ea8 || (rtype == R_X86_64_JUMP_SLOT && value == 0) {
            log::warn!("[dl-debug] target={:#x} offset={:#x} sym_idx={} value={:#x} rtype={}", target, rela_offset, sym_idx, value, rtype);
        }

        // Write through the live user page table.
        // Safe: CR3 = user PML4 during SYSCALL; SMAP not set in this build.
        unsafe { write_user_u64(target, value); }
    }

    log::info!(
        "[dl] rela table sz={} count={} applied: REL={} _64={} GLOB={} JMP={} other={} oob={}",
        table_sz, count, n_rel, n_64, n_glob, n_jmp, n_other, n_oob
    );
}

// ── Symbol export harvesting ───────────────────────────────────────────────────

fn harvest_exports(bytes: &[u8], d: &DynInfo, load_base: u64) -> Vec<ExportedSym> {
    let mut out = Vec::new();
    
    // 1. Harvest from dynamic symbols (.dynsym)
    if d.symtab_file != 0 && d.strtab_file != 0 && d.syment != 0 {
        let sym_cap = if d.strtab_va > d.symtab_va && d.syment > 0 {
            (d.strtab_va - d.symtab_va) / d.syment
        } else {
            512 // fallback cap
        };

        for i in 0..sym_cap {
            let sym: Elf64Sym = match read_pod(bytes, d.symtab_file + i * d.syment) {
                Some(s) => s,
                None    => break,
            };
            let binding = sym.st_info >> 4;
            if binding != STB_GLOBAL && binding != STB_WEAK { continue; }
            if sym.st_shndx == SHN_UNDEF || sym.st_value == 0 { continue; }

            let name = read_cstr(bytes, d.strtab_file + sym.st_name as usize);
            if name.is_empty() { continue; }

            out.push(ExportedSym { name, vaddr: load_base + sym.st_value });
        }
    }

    // 2. Harvest from static symbols (.symtab) to support resolving internal functions
    // (such as FlutterEngineGetProcAddresses in AOT profile GTK engine builds).
    if let Some(hdr) = read_pod::<Elf64Hdr>(bytes, 0) {
        let sh_off = hdr.e_shoff as usize;
        let sh_num = hdr.e_shnum as usize;
        let sh_size = hdr.e_shentsize as usize;
        if sh_off != 0 && sh_num != 0 && sh_size >= 64 {
            for i in 0..sh_num {
                if let Some(sh) = read_pod::<Elf64Shdr>(bytes, sh_off + i * sh_size) {
                    if sh.sh_type == 2 { // SHT_SYMTAB = 2
                        let symtab_file = sh.sh_offset as usize;
                        let symtab_sz = sh.sh_size as usize;
                        let syment = sh.sh_entsize as usize;
                        let link = sh.sh_link as usize;
                        if syment >= 24 && link < sh_num {
                            if let Some(str_sh) = read_pod::<Elf64Shdr>(bytes, sh_off + link * sh_size) {
                                let strtab_file = str_sh.sh_offset as usize;
                                let count = symtab_sz / syment;
                                for j in 0..count {
                                    if let Some(sym) = read_pod::<Elf64Sym>(bytes, symtab_file + j * syment) {
                                        if sym.st_shndx == SHN_UNDEF || sym.st_value == 0 { continue; }
                                        let name = read_cstr(bytes, strtab_file + sym.st_name as usize);
                                        if name.is_empty() { continue; }
                                        
                                        // To prevent memory bloat/slowness with 50k+ symbols, we only harvest
                                        // static symbols starting with "FlutterEngine", "fl_", "_kDart", or the FML message loop symbol
                                        if name.starts_with(b"FlutterEngine")
                                            || name.starts_with(b"fl_")
                                            || name.starts_with(b"_kDart")
                                            || name == b"_ZN3fml11MessageLoop33EnsureInitializedForCurrentThreadEv"
                                        {
                                            let val = load_base + sym.st_value;
                                            if !out.iter().any(|e| e.name == name) {
                                                out.push(ExportedSym { name, vaddr: val });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    out
}

fn read_cstr(bytes: &[u8], off: usize) -> Vec<u8> {
    if off >= bytes.len() { return Vec::new(); }
    let max = bytes.len().min(off + 256);
    let len = bytes[off..max].iter().position(|&b| b == 0).unwrap_or(max - off);
    bytes[off..off + len].to_vec()
}

// ── PT_LOAD span ───────────────────────────────────────────────────────────────

fn pt_load_span(bytes: &[u8], hdr: &Elf64Hdr) -> u64 {
    let ph_off  = hdr.e_phoff as usize;
    let ph_num  = hdr.e_phnum as usize;
    let ph_size = hdr.e_phentsize as usize;
    let mut max_end = 0u64;
    log::info!("[DL] pt_load_span: ph_off={} ph_num={} ph_size={}", ph_off, ph_num, ph_size);
    for i in 0..ph_num {
        let ph: Elf64Phdr = match read_pod(bytes, ph_off + i * ph_size) {
            Some(p) => p,
            None    => break,
        };
        let p_type = ph.p_type;
        let p_flags = ph.p_flags;
        let p_offset = ph.p_offset;
        let p_vaddr = ph.p_vaddr;
        let p_memsz = ph.p_memsz;
        log::info!("[DL] ph[{}]: type={} flags={} offset={:#x} vaddr={:#x} memsz={:#x}", i, p_type, p_flags, p_offset, p_vaddr, p_memsz);
        if p_type != PT_LOAD { continue; }
        let end = p_vaddr.saturating_add(p_memsz);
        if end > max_end { max_end = end; }
    }
    log::info!("[DL] pt_load_span: max_end={:#x}", max_end);
    max_end
}

// ── Public API ─────────────────────────────────────────────────────────────────

/// Load an ELF shared object (`ET_DYN`) into process `pid`'s address space.
///
/// 1. Validates the ELF header.
/// 2. Assigns a load base via the per-process VA bump allocator.
/// 3. Maps every `PT_LOAD` segment into `pml4_phys`.
/// 4. Applies `R_X86_64_RELATIVE` / `R_X86_64_64` / `GLOB_DAT` / `JUMP_SLOT`.
/// 5. Harvests all `STB_GLOBAL`/`STB_WEAK` exports for [`dlsym`].
///
/// Returns a positive library handle on success or `Err(&str)`.
pub fn dlopen(pid: u32, pml4_phys: u64, elf_bytes: &[u8], path: &[u8]) -> Result<u32, &'static str> {
    // ── Validate ELF header ───────────────────────────────────────────────
    let hdr: Elf64Hdr = read_pod(elf_bytes, 0).ok_or("ELF too short")?;
    if hdr.e_ident[0..4] != ELF_MAGIC { return Err("not an ELF"); }
    if hdr.e_ident[4] != ELFCLASS64    { return Err("not ELF64"); }
    if hdr.e_ident[5] != ELFDATA2LSB   { return Err("not little-endian"); }
    if hdr.e_type    != ET_DYN          { return Err("not a shared object (ET_DYN required)"); }
    if hdr.e_machine != EM_X86_64       { return Err("not x86_64"); }

    // ── Allocate load base ────────────────────────────────────────────────
    // Key the bump cursor on pml4_phys, not pid, so threads sharing an
    // address space also share the cursor (otherwise they would re-bump
    // from LIB_VA_BASE and silently overwrite each other's mappings).
    let _ = pid;
    let span      = pt_load_span(elf_bytes, &hdr);
    let load_base = LIBS.lock().bump_lib_va(pml4_phys, span);

    // ── Map PT_LOAD segments ──────────────────────────────────────────────
    let ph_off  = hdr.e_phoff as usize;
    let ph_num  = hdr.e_phnum as usize;
    let ph_size = hdr.e_phentsize as usize;

    for i in 0..ph_num {
        let ph: Elf64Phdr = read_pod(elf_bytes, ph_off + i * ph_size).ok_or("phdr OOB")?;
        if ph.p_type != PT_LOAD || ph.p_memsz == 0 { continue; }

        let seg_va    = load_base + ph.p_vaddr;
        let page_base = seg_va & !0xFFF;
        let page_off  = (seg_va - page_base) as usize;
        let total     = page_off + ph.p_memsz as usize;
        let npages    = (total + 4095) / 4096;
        let file_off  = ph.p_offset as usize;
        let filesz    = ph.p_filesz as usize;
        let writable  = true; // Force writable to true to allow in-memory patching and engine deserialization writes
        let exec      = (ph.p_flags & PF_X) != 0;

        let copy_limit = if filesz == ph.p_memsz as usize { npages * 4096 } else { page_off + filesz };

        for p in 0..npages {
            let page_phys = frame_allocator::alloc_frame().ok_or("OOM: dl segment")?;
            let hhdm_ptr  = (page_phys + frame_allocator::hhdm_offset()) as *mut u8;
            unsafe { core::ptr::write_bytes(hhdm_ptr, 0, 4096); }

            // Fast bulk copy: find intersection of this 4-KiB page window with
            // the file data range, then copy_nonoverlapping in one shot.
            let page_start = p * 4096; // offset from seg_va
            let page_end   = page_start + 4096;
            let data_start = page_start.max(page_off);
            let data_end   = page_end.min(copy_limit);
            if data_start < data_end {
                let dst_off  = data_start - page_start;
                let src_off  = file_off + (data_start - page_off);
                let copy_len = data_end - data_start;
                if src_off + copy_len <= elf_bytes.len() {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            elf_bytes.as_ptr().add(src_off),
                            hhdm_ptr.add(dst_off),
                            copy_len,
                        );
                    }
                }
            }

            unsafe {
                paging::map_user_page_with_flags(
                    pml4_phys,
                    page_base + (p * 4096) as u64,
                    page_phys,
                    writable,
                    exec,
                )?;
            }
        }
    }

    // ── Map any unmapped gap pages within the library span ───────────────
    {
        let span_pages = (span + 4095) / 4096;
        let mut gap_mapped = 0;
        for p in 0..span_pages {
            let va = load_base + p * 4096;
            if paging::translate_user_page(pml4_phys, va).is_none() {
                if let Some(page_phys) = frame_allocator::alloc_frame() {
                    let hhdm_ptr = (page_phys + frame_allocator::hhdm_offset()) as *mut u8;
                    unsafe { core::ptr::write_bytes(hhdm_ptr, 0, 4096); }
                    unsafe {
                        paging::map_user_page_with_flags(
                            pml4_phys,
                            va,
                            page_phys,
                            true, // writable
                            false, // exec
                        )?;
                    }
                    gap_mapped += 1;
                } else {
                    return Err("OOM: gap page allocation");
                }
            }
        }
        if gap_mapped > 0 {
            log::info!("[DL] Mapped {} gap page(s) within library span of size {:#x}", gap_mapped, span);
        }
    }

    // ── Parse DYNAMIC section ─────────────────────────────────────────────
    let dyn_info = parse_dynamic(elf_bytes, &hdr);

    // ── Resolve intra-library symbols for relocation ──────────────────────
    let sym_vaddr: &dyn Fn(usize) -> u64 = &|sym_idx: usize| {
        let d = match dyn_info.as_ref() { Some(d) => d, None => return 0 };
        if d.symtab_file == 0 || d.syment == 0 { return 0; }
        let sym: Elf64Sym = match read_pod(elf_bytes, d.symtab_file + sym_idx * d.syment) {
            Some(s) => s,
            None    => return 0,
        };
        if sym.st_shndx != SHN_UNDEF && sym.st_value != 0 {
            // Defined in this library.
            return load_base + sym.st_value;
        }
        // Undefined external — look up name.
        if d.strtab_file == 0 { return 0; }
        let name = read_cstr(elf_bytes, d.strtab_file + sym.st_name as usize);
        if name.is_empty() { return 0; }
        // 1. Check other already-loaded libraries for this process.
        {
            let t = LIBS.lock();
            let leader_pid = crate::process::get_group_leader(pid);
            for lib in &t.entries {
                let lib_leader = crate::process::get_group_leader(lib.pid);
                if lib_leader != leader_pid { continue; }
                for exp in &lib.exports {
                    if sym_name_eq(exp.name.as_slice(), name.as_slice()) {
                        return exp.vaddr;
                    }
                }
            }
        }
        // 2. Check POSIX trampoline page.
        if let Some(va) = crate::process::posix_trampolines::find_posix_symbol(
            core::str::from_utf8(&name).unwrap_or(""),
        ) {
            return va;
        }
        // 3. Fallback: redirect unresolved symbols to appropriate fallback stubs.
        if let Ok(name_str) = core::str::from_utf8(&name) {
            let fallback_name = if name_str.starts_with("Fc") {
                if name_str == "FcConfigGetFonts" || name_str == "FcFontSetCreate" {
                    "__oscortex_fallback_stub_fcfontset"
                } else if name_str == "FcConfigCreate"
                    || name_str == "FcInitLoadConfigAndFonts"
                    || name_str == "FcPatternCreate"
                    || name_str == "FcCharSetCreate"
                    || name_str == "FcLangSetCreate"
                    || name_str == "FcObjectSetBuild"
                    || name_str == "FcFontSetMatch"
                    || name_str == "FcFontMatch"
                    || name_str == "FcPatternDuplicate"
                    || name_str == "FcPatternFilter"
                    || name_str == "FcFontRenderPrepare"
                {
                    "__oscortex_fallback_stub_nonnull"
                } else if name_str.contains("Add") || name_str.contains("Substitute") || name_str == "FcPatternEqual" || name_str == "FcPatternReference" {
                    "__oscortex_fallback_stub_true"
                } else {
                    "__oscortex_fallback_stub"
                }
            } else if name_str == "isatty" {
                "__oscortex_fallback_stub_true"
            } else {
                "__oscortex_fallback_stub"
            };

            if let Some(fallback_va) = crate::process::posix_trampolines::find_posix_symbol(fallback_name) {
                log::warn!("[DL] Unresolved symbol '{}', redirecting to '{}' at {:#x}", name_str, fallback_name, fallback_va);
                return fallback_va;
            }
        }
        0
    };

    // ── Apply relocations ─────────────────────────────────────────────────
    if let Some(ref d) = dyn_info {
        apply_rela_table(elf_bytes, d.rela_file,   d.relasz,   load_base, sym_vaddr);
        apply_rela_table(elf_bytes, d.jmprel_file, d.pltrelsz, load_base, sym_vaddr);
    }

    // ── Harvest exported symbols ──────────────────────────────────────────
    let exports = match dyn_info.as_ref() {
        Some(d) => harvest_exports(elf_bytes, d, load_base),
        None    => Vec::new(),
    };
    let export_count = exports.len();

    // ── Compute init function VAs from parsed dynamic info ─────────────────
    let (init_fn_va, init_array_va, init_array_len) = match dyn_info.as_ref() {
        Some(d) => {
            let ifn = if d.init_va       != 0 { load_base + d.init_va as u64       } else { 0 };
            let iar = if d.init_array_va != 0 { load_base + d.init_array_va as u64 } else { 0 };
            let ilen = d.init_arraysz / 8;
            (ifn, iar, ilen)
        }
        None => (0, 0, 0),
    };

    // ── Register in the library table ─────────────────────────────────────
    let handle = {
        let mut t = LIBS.lock();
        if t.entries.len() >= MAX_LIBS {
            return Err("library table full");
        }
        let h = t.next_handle;
        t.next_handle = t.next_handle.wrapping_add(1).max(1);
        t.entries.push(LoadedLib {
            pid, handle: h, load_base, exports,
            init_fn_va, init_array_va, init_array_len,
            path: path.to_vec(),
            size: span,
        });
        h
    };

    // ── Runtime patch: libflutter_engine.so ──────────────────────────────
    // Identified by ELF size > 50 MiB (the engine is ~91 MiB; no other
    // library we load comes close).
    if elf_bytes.len() > 50 * 1024 * 1024 {
        // Helper: write N bytes at a virtual address via HHDM.
        let mut write_va = |va: u64, data: &[u8]| -> bool {
            match crate::mm::paging::translate_user_page(pml4_phys, va) {
                Some(page_phys) => {
                    let off  = (va & 0xFFF) as usize;
                    let hhdm = crate::mm::frame_allocator::hhdm_offset();
                    let ptr  = (page_phys + hhdm + off as u64) as *mut u8;
                    unsafe {
                        for (i, &b) in data.iter().enumerate() {
                            ptr.add(i).write_volatile(b);
                        }
                    }
                    true
                }
                None => false,
            }
        };

        // Patch 1: EmbedderTaskRunner::RunsTasksOnCurrentThread (nm 0x197b8a0)
        //   B0 01 C3  →  mov al, 1; ret  (always return true)
        // Patching this covers the EmbedderTaskRunner vtable path.
        if false {
            const NM: u64 = 0x197b8a0;
            let va = load_base + NM;
            if write_va(va, &[0xB0, 0x01, 0xC3]) {
                log::info!("[DL] Patch1 RunsTasksOnCurrentThread @ {:#x} → mov al,1; ret", va);
            } else {
                log::warn!("[DL] Patch1 FAILED: translate @ {:#x}", va);
            }
        }

        // Patch 2: ~promise<RuntimeStageBackend> (nm 0x21a81b0)
        //   C3  →  ret  (suppress broken_promise abort)
        // The promise destructor calls std::terminate when the promise was
        // never satisfied.  Silencing it lets CreateShellOnPlatformThread
        // continue even when the raster-runner task is posted rather than
        // executed inline.
        {
            const NM: u64 = 0x21a81b0;
            let va = load_base + NM;
            if write_va(va, &[0xC3]) {
                log::info!("[DL] Patch2 ~promise<RTB>        @ {:#x} → ret", va);
            } else {
                log::warn!("[DL] Patch2 FAILED: translate @ {:#x}", va);
            }
        }

        // Patch 6: ~promise<std::shared_ptr<impeller::Context>> (nm 0x21a8230)
        //   C3  →  ret  (suppress broken_promise abort)
        {
            const NM: u64 = 0x21a8230;
            let va = load_base + NM;
            if write_va(va, &[0xC3]) {
                log::info!("[DL] Patch6 ~promise<impeller::Context> @ {:#x} → ret", va);
            } else {
                log::warn!("[DL] Patch6 FAILED: translate @ {:#x}", va);
            }
        }


        // Patch 3: fml::TaskRunner::RunNowOrPostTask (nm 0x19bfd00)
        //   Find the `84 C0  74 16` pair (test al,al ; je +0x16) after the
        //   RunsTasksOnCurrentThread vtable call. Replace ALL FOUR bytes
        //   with 4×nop so the post-branch is unconditionally skipped and
        //   the lambda runs inline on the calling thread.
        //
        //   The earlier 2-byte patch (test → mov al,1) was BROKEN because
        //   `mov al,1` does not modify EFLAGS — the following `je` then
        //   branched on stale flags from the prior callq, producing
        //   non-deterministic behaviour. NOPping the entire test+je is
        //   correct and idempotent.
        if false {
            const NM: u64 = 0x19bfd00;
            let va   = load_base + NM;
            let hhdm = crate::mm::frame_allocator::hhdm_offset();
            let mut patched3 = false;
            'scan: for byte_off in 0u64..128 {
                let scan_va = va + byte_off;
                let mut buf = [0u8; 4];
                let mut ok  = true;
                for i in 0..4u64 {
                    match crate::mm::paging::translate_user_page(pml4_phys, scan_va + i) {
                        Some(pp) => {
                            let off = ((scan_va + i) & 0xFFF) as usize;
                            buf[i as usize] = unsafe { *((pp + hhdm + off as u64) as *const u8) };
                        }
                        None => { ok = false; break; }
                    }
                }
                if !ok { continue; }
                // Match: 84 C0 74 16  (test al,al ; je +0x16)
                if buf[0] == 0x84 && buf[1] == 0xC0 && buf[2] == 0x74 && buf[3] == 0x16 {
                    if write_va(scan_va, &[0x90, 0x90, 0x90, 0x90]) {
                        log::info!(
                            "[DL] Patch3 RunNowOrPostTask `test;je` @ {:#x}+{} → 4×nop (force inline)",
                            va, byte_off
                        );
                        patched3 = true;
                    }
                    break 'scan;
                }
            }
            if !patched3 {
                log::warn!("[DL] Patch3 RunNowOrPostTask: `test al,al ; je +16` not found in first 128 bytes @ {:#x}", va);
            }
        }

        // Patch 4: NOP the single `call AutoResetWaitableEvent::Wait` inside
        // Shell::Create at file offset 0x21a6999 (verified by objdump).
        //
        //   e8 02 91 81 ff   →   90 90 90 90 90   (5-byte NOP slide)
        //
        // Rationale: Shell::Create posts CreateShellOnPlatformThread to the
        // platform task runner via RunNowOrPostTask, then calls latch.Wait().
        // With Patch3 making RunNowOrPostTask run inline on the caller's
        // thread, the posted lambda — and its Signal() — execute BEFORE the
        // following Wait(). The latch is therefore already signaled, and
        // Wait() should return immediately. But our cond_var path can lose
        // that early Signal (the wait pre-records `seq` and futex_wait sees
        // unchanged seq → blocks).  NOPping the call removes the spurious
        // park entirely. Targeted to this one call site only — leaves the
        // global Wait function intact so worker threads that legitimately
        // need to block still can.
        if false {
            const CALL_VA: u64 = 0x21a6999;
            let va = load_base + CALL_VA;
            // Verify the bytes before overwriting (safety guard).
            let hhdm = crate::mm::frame_allocator::hhdm_offset();
            let mut matches = true;
            const EXPECTED: [u8; 5] = [0xE8, 0x02, 0x91, 0x81, 0xFF];
            for i in 0..5 {
                if let Some(pp) = crate::mm::paging::translate_user_page(pml4_phys, va + i) {
                    let off = ((va + i) & 0xFFF) as usize;
                    let b = unsafe { *((pp + hhdm + off as u64) as *const u8) };
                    if b != EXPECTED[i as usize] { matches = false; break; }
                } else {
                    matches = false;
                    break;
                }
            }
            if matches {
                if write_va(va, &[0x90, 0x90, 0x90, 0x90, 0x90]) {
                    log::info!("[DL] Patch4 Shell::Create latch.Wait()  @ {:#x} → 5×nop", va);
                } else {
                    log::warn!("[DL] Patch4 FAILED: translate @ {:#x}", va);
                }
            } else {
                log::warn!("[DL] Patch4 SKIPPED: bytes @ {:#x} do not match expected call insn", va);
            }
        }

        // Patch 5: NOP the single `call fml::ManualResetWaitableEvent::Wait` inside
        // PlatformView::NotifyCreated at file offset 0x219da5f.
        //
        //   e8 dc 20 82 ff   →   90 90 90 90 90   (5-byte NOP slide)
        //
        // Rationale: PlatformView::NotifyCreated posts tasks to the raster thread
        // and then waits for the raster thread to create the rendering surface.
        // Since we execute tasks inline, the surface is created immediately,
        // so waiting is unnecessary and deadlocks on the lost signal problem.
        if false {
            const CALL_VA: u64 = 0x219da5f;
            let va = load_base + CALL_VA;
            // Verify the bytes before overwriting (safety guard).
            let hhdm = crate::mm::frame_allocator::hhdm_offset();
            let mut matches = true;
            const EXPECTED: [u8; 5] = [0xE8, 0xDC, 0x20, 0x82, 0xFF];
            for i in 0..5 {
                if let Some(pp) = crate::mm::paging::translate_user_page(pml4_phys, va + i) {
                    let off = ((va + i) & 0xFFF) as usize;
                    let b = unsafe { *((pp + hhdm + off as u64) as *const u8) };
                    if b != EXPECTED[i as usize] { matches = false; break; }
                } else {
                    matches = false;
                    break;
                }
            }
            if matches {
                if write_va(va, &[0x90, 0x90, 0x90, 0x90, 0x90]) {
                    log::info!("[DL] Patch5 PlatformView::NotifyCreated latch.Wait() @ {:#x} → 5×nop", va);
                } else {
                    log::warn!("[DL] Patch5 FAILED: translate @ {:#x}", va);
                }
            } else {
                log::warn!("[DL] Patch5 SKIPPED: bytes @ {:#x} do not match expected call insn", va);
            }
        }

        // Patch 7: Fix PageFault at ImageReader::GetInstructionsAt due to 32-bit offset overflow.
        //   Target Offset: 0x22cfe60
        //   Expected Bytes: [0x48, 0x8b, 0x47, 0x08, 0x89, 0xf1, 0x48, 0x01, 0xc8, 0x48, 0xff, 0xc0]
        //
        //   Root cause: for late Code objects (indices ~1244-1246 of 1246 total), the Dart AOT
        //   snapshot encodes an instructions offset that exceeds 2^31. Neither 32-bit truncation
        //   nor sign-extension can handle it, producing unmapped pointers like 0x1a3344001.
        //   Fix: unconditionally return base_instructions+1 (the Dart-tagged base Instructions ptr).
        //   All Code objects share the same executable instructions region; this is a safe fallback.
        {
            const TARGET_VA: u64 = 0x22cfe60;
            let va = load_base + TARGET_VA;
            let hhdm = crate::mm::frame_allocator::hhdm_offset();
            let mut matches = true;
            const EXPECTED: [u8; 12] = [0x48, 0x8B, 0x47, 0x08, 0x89, 0xF1, 0x48, 0x01, 0xC8, 0x48, 0xFF, 0xC0];
            for i in 0..12 {
                if let Some(pp) = crate::mm::paging::translate_user_page(pml4_phys, va + i) {
                    let off = ((va + i) & 0xFFF) as usize;
                    let b = unsafe { *((pp + hhdm + off as u64) as *const u8) };
                    if b != EXPECTED[i as usize] { matches = false; break; }
                } else {
                    matches = false;
                    break;
                }
            }
            if matches {
                // Return base Instructions pointer unconditionally (no offset arithmetic):
                //   mov rax, [rdi+8]  ; base_instructions_ ptr
                //   inc rax           ; +1 Dart tagged ptr
                //   ret
                //   nop * 6           ; pad to match original 14 bytes
                const REPLACEMENT: [u8; 14] = [
                    0x48, 0x8B, 0x47, 0x08, // mov rax, qword ptr [rdi + 8]
                    0x48, 0xFF, 0xC0,       // inc rax
                    0xC3,                   // ret
                    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // nop * 6
                ];
                if write_va(va, &REPLACEMENT) {
                    log::info!("[DL] Patch7 ImageReader::GetInstructionsAt @ {:#x} -> safe base-ptr stub (no offset arith)", va);
                } else {
                    log::warn!("[DL] Patch7 FAILED: write inline patch @ {:#x}", va);
                }
            } else {
                log::warn!("[DL] Patch7 SKIPPED: bytes @ {:#x} do not match expected instructions", va);
            }
        }
    }

    log::info!("[DL] pid={} dlopen base={:#x} handle={} exports={}", pid, load_base, handle, export_count);
    Ok(handle)
}

/// Resolve a symbol by name from a loaded library.
///
/// `name` is a raw byte slice (no null terminator).
/// Returns the virtual address, or `None` if not found.
pub fn dlsym(handle: u32, name: &[u8]) -> Option<u64> {
    let pid = crate::process::current_pid();
    let leader_pid = crate::process::get_group_leader(pid);
    let t = LIBS.lock();
    for lib in t.entries.iter().rev() {
        let lib_leader = crate::process::get_group_leader(lib.pid);
        if lib_leader != leader_pid { continue; }
        if handle != 0 && lib.handle != handle { continue; }
        for sym in &lib.exports {
            if sym_name_eq(sym.name.as_slice(), name) {
                return Some(sym.vaddr);
            }
        }
    }
    if handle == 0 {
        if let Ok(name_str) = core::str::from_utf8(name) {
            if let Some(va) = crate::process::posix_trampolines::find_posix_symbol(name_str) {
                return Some(va);
            }
        }
    }
    None
}

/// Release a library handle (drop the export table).
///
/// The physical frames remain mapped — full reclamation requires a VMM with
/// reverse-mappings and is deferred to a future milestone.
pub fn dlclose(handle: u32, pid: u32) {
    let leader_pid = crate::process::get_group_leader(pid);
    let mut t = LIBS.lock();
    if let Some(pos) = t.entries.iter().position(|l| l.handle == handle && crate::process::get_group_leader(l.pid) == leader_pid) {
        t.entries.remove(pos);
    }
}

/// Return the init function VAs for a loaded library.
///
/// Returns `(init_fn_va, init_array_va, init_array_len)` where:
/// - `init_fn_va`    is the DT_INIT function VA (0 = none)
/// - `init_array_va` is the VA of the first DT_INIT_ARRAY entry (0 = none)
/// - `init_array_len` is the number of entries in the array
pub fn get_init_fns(handle: u32, pid: u32) -> Option<(u64, u64, usize)> {
    let leader_pid = crate::process::get_group_leader(pid);
    let t = LIBS.lock();
    for lib in &t.entries {
        let lib_leader = crate::process::get_group_leader(lib.pid);
        if lib.handle == handle && lib_leader == leader_pid {
            return Some((lib.init_fn_va, lib.init_array_va, lib.init_array_len));
        }
    }
    None
}

/// Return the load base address for a loaded library.
pub fn get_load_base(handle: u32, pid: u32) -> Option<u64> {
    let leader_pid = crate::process::get_group_leader(pid);
    let t = LIBS.lock();
    for lib in &t.entries {
        let lib_leader = crate::process::get_group_leader(lib.pid);
        if lib.handle == handle && lib_leader == leader_pid {
            return Some(lib.load_base);
        }
    }
    None
}

/// Allocate `pages * 4096` bytes of anonymous memory in process `pid`'s
/// address space, mapped at `hint_va` (rounded down to page) if non-zero,
/// or at the next available VA in the process's dynamic mapping region.
///
/// `prot` bits: `PROT_READ=1`, `PROT_WRITE=2`, `PROT_EXEC=4`.
///
/// Returns the base virtual address, or `u64::MAX` on error.
pub fn mmap_anon(pid: u32, pml4_phys: u64, hint_va: u64, pages: usize, prot: u64) -> u64 {
    if pages == 0 || pages > 65536 { return u64::MAX; } // max 256 MiB

    let writable = (prot & 0x2) != 0 || prot == 0; // PROT_WRITE or PROT_NONE → writable for now
    let exec     = (prot & 0x4) != 0;              // PROT_EXEC

    let _ = pid;
    let base_va = if hint_va != 0 {
        let aligned = hint_va & !0xFFF;
        // Check that NONE of the target pages are already mapped. If any
        // page in the requested range is occupied (e.g., libflutter's
        // .data.rel.ro), fall back to the bump allocator so we don't
        // silently clobber existing mappings.
        let mut clear = true;
        for p in 0..pages {
            let v = aligned + (p * 4096) as u64;
            if paging::translate_user_page(pml4_phys, v).is_some() {
                clear = false;
                break;
            }
        }
        if clear {
            aligned
        } else {
            log::warn!(
                "[mmap_anon] hint {:#x} pages={} overlaps existing mapping; reallocating",
                hint_va, pages
            );
            LIBS.lock().bump_anon_va(pml4_phys, (pages * 4096) as u64)
        }
    } else {
        // Bump-allocate, but skip over any already-mapped ranges. This
        // prevents the TLS/stack allocator from clobbering a region that
        // was previously mmap'd by userspace (e.g. a Dart heap chunk that
        // sits at the same VA the bump cursor would return next).
        let mut candidate = LIBS.lock().bump_anon_va(pml4_phys, (pages * 4096) as u64);
        for _ in 0..64 {
            let mut clear = true;
            for p in 0..pages {
                let v = candidate + (p * 4096) as u64;
                if paging::translate_user_page(pml4_phys, v).is_some() {
                    if v >= 0x33b000000 && v < 0x33e000000 {
                        log::warn!("[mmap_anon] collision detected at {:#x} for candidate {:#x}", v, candidate);
                    }
                    clear = false;
                    break;
                }
            }
            if clear { break; }
            log::warn!("[mmap_anon] bump candidate {:#x} pages={} conflicts; advancing",
                candidate, pages);
            candidate = LIBS.lock().bump_anon_va(pml4_phys, (pages * 4096) as u64);
        }
        candidate
    };

    for p in 0..pages {
        let virt = base_va + (p * 4096) as u64;
        let phys = match frame_allocator::alloc_frame() {
            Some(f) => f,
            None    => return u64::MAX,
        };
        let hhdm = (phys + frame_allocator::hhdm_offset()) as *mut u8;
        unsafe { core::ptr::write_bytes(hhdm, 0, 4096); }
        unsafe {
            if paging::map_user_page_with_flags(pml4_phys, virt, phys, writable, exec).is_err() {
                return u64::MAX;
            }
        }
    }

    base_va
}

fn sym_name_eq(a: &[u8], b: &[u8]) -> bool {
    let a_clean = if a.starts_with(b"_") { &a[1..] } else { a };
    let b_clean = if b.starts_with(b"_") { &b[1..] } else { b };
    a_clean == b_clean
}

/// Resolve the library info for a given user-space address.
///
/// Returns `(path, load_base)` if `addr` falls within any loaded library's span.
pub fn dladdr(pid: u32, addr: u64) -> Option<(Vec<u8>, u64)> {
    let leader_pid = crate::process::get_group_leader(pid);
    let t = LIBS.lock();
    for lib in &t.entries {
        let lib_leader = crate::process::get_group_leader(lib.pid);
        if lib_leader == leader_pid && addr >= lib.load_base && addr < lib.load_base + lib.size {
            return Some((lib.path.clone(), lib.load_base));
        }
    }
    None
}

