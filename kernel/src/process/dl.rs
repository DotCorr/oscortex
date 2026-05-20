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
const LIB_VA_BASE:   u64 = 0x0100_0000; // 16 MiB
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
}

// ── Global library table ───────────────────────────────────────────────────────

/// Maximum number of distinct address spaces we track bump cursors for.
const MAX_AS_SLOTS: usize = 64;

struct LibTable {
    entries:     Vec<LoadedLib>,
    next_handle: u32,
    /// Per-address-space bump VA for the next dynamic mapping (libraries + mmap).
    /// Keyed by `pml4_phys` so that threads sharing an address space also share
    /// the cursor — otherwise each thread would re-bump from `LIB_VA_BASE` and
    /// silently overwrite its siblings' heap mappings in the shared PML4.
    /// Fixed-size linear table: (pml4_phys, next_va). `pml4_phys == 0` ⇒ free.
    as_slots:    [(u64, u64); MAX_AS_SLOTS],
}

impl LibTable {
    const fn new() -> Self {
        Self {
            entries:     Vec::new(),
            next_handle: 1,
            as_slots:    [(0, 0); MAX_AS_SLOTS],
        }
    }

    fn bump_va(&mut self, pml4_phys: u64, size: u64) -> u64 {
        let aligned = (size + LIB_VA_STRIDE - 1) / LIB_VA_STRIDE * LIB_VA_STRIDE;
        // Find existing slot for this address space.
        for slot in self.as_slots.iter_mut() {
            if slot.0 == pml4_phys {
                let base = slot.1;
                slot.1 = base + aligned;
                return base;
            }
        }
        // Allocate a new slot.
        for slot in self.as_slots.iter_mut() {
            if slot.0 == 0 {
                *slot = (pml4_phys, LIB_VA_BASE + aligned);
                return LIB_VA_BASE;
            }
        }
        // Fallback: should not happen with MAX_AS_SLOTS=64. Reuse slot 0.
        let base = self.as_slots[0].1;
        self.as_slots[0].1 = base + aligned;
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

    for i in 0..count {
        let rela: Elf64Rela = match read_pod(bytes, rela_file + i * ent_sz) {
            Some(r) => r,
            None    => break,
        };
        let rtype   = (rela.r_info & 0xFFFF_FFFF) as u32;
        let sym_idx = (rela.r_info >> 32) as usize;
        let target  = load_base.wrapping_add(rela.r_offset);

        let value = match rtype {
            R_X86_64_RELATIVE  => load_base.wrapping_add(rela.r_addend as u64),
            R_X86_64_64        => sym_vaddr(sym_idx).wrapping_add(rela.r_addend as u64),
            R_X86_64_GLOB_DAT  => sym_vaddr(sym_idx),
            R_X86_64_JUMP_SLOT => sym_vaddr(sym_idx),
            0                  => continue, // R_X86_64_NONE
            _                  => continue, // unsupported — leave as zero
        };

        // Write through the live user page table.
        // Safe: CR3 = user PML4 during SYSCALL; SMAP not set in this build.
        unsafe { write_user_u64(target, value); }
    }
}

// ── Symbol export harvesting ───────────────────────────────────────────────────

fn harvest_exports(bytes: &[u8], d: &DynInfo, load_base: u64) -> Vec<ExportedSym> {
    let mut out = Vec::new();
    if d.symtab_file == 0 || d.strtab_file == 0 || d.syment == 0 { return out; }

    // Estimate symbol count from the VA gap between symtab and strtab.
    // This is a well-established heuristic: GNU ld places .dynsym immediately
    // before .dynstr in most shared libraries.
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
    for i in 0..ph_num {
        let ph: Elf64Phdr = match read_pod(bytes, ph_off + i * ph_size) {
            Some(p) => p,
            None    => break,
        };
        if ph.p_type != PT_LOAD { continue; }
        let end = ph.p_vaddr.saturating_add(ph.p_memsz);
        if end > max_end { max_end = end; }
    }
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
pub fn dlopen(pid: u32, pml4_phys: u64, elf_bytes: &[u8]) -> Result<u32, &'static str> {
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
    let load_base = LIBS.lock().bump_va(pml4_phys, span);

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
        let writable  = (ph.p_flags & PF_W) != 0;
        let exec      = (ph.p_flags & PF_X) != 0;

        for p in 0..npages {
            let page_phys = frame_allocator::alloc_frame().ok_or("OOM: dl segment")?;
            let hhdm_ptr  = (page_phys + frame_allocator::hhdm_offset()) as *mut u8;
            unsafe { core::ptr::write_bytes(hhdm_ptr, 0, 4096); }

            // Fast bulk copy: find intersection of this 4-KiB page window with
            // the file data range, then copy_nonoverlapping in one shot.
            let page_start = p * 4096; // offset from seg_va
            let page_end   = page_start + 4096;
            let data_start = page_start.max(page_off);
            let data_end   = page_end.min(page_off + filesz);
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
        // 1. Check POSIX trampoline page.
        if let Some(va) = crate::process::posix_trampolines::find_posix_symbol(
            core::str::from_utf8(&name).unwrap_or(""),
        ) {
            return va;
        }
        // 2. Check other already-loaded libraries for this process.
        {
            let t = LIBS.lock();
            for lib in &t.entries {
                if lib.pid != pid { continue; }
                for exp in &lib.exports {
                    if exp.name.as_slice() == name.as_slice() {
                        return exp.vaddr;
                    }
                }
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
        });
        h
    };

    log::info!("[DL] pid={} dlopen base={:#x} handle={} exports={}", pid, load_base, handle, export_count);
    Ok(handle)
}

/// Resolve a symbol by name from a loaded library.
///
/// `name` is a raw byte slice (no null terminator).
/// Returns the virtual address, or `None` if not found.
pub fn dlsym(handle: u32, name: &[u8]) -> Option<u64> {
    let t = LIBS.lock();
    for lib in &t.entries {
        if lib.handle != handle { continue; }
        for sym in &lib.exports {
            if sym.name.as_slice() == name {
                return Some(sym.vaddr);
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
    let mut t = LIBS.lock();
    if let Some(pos) = t.entries.iter().position(|l| l.handle == handle && l.pid == pid) {
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
    let t = LIBS.lock();
    for lib in &t.entries {
        if lib.handle == handle && lib.pid == pid {
            return Some((lib.init_fn_va, lib.init_array_va, lib.init_array_len));
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
        hint_va & !0xFFF // align down
    } else {
        LIBS.lock().bump_va(pml4_phys, (pages * 4096) as u64)
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

