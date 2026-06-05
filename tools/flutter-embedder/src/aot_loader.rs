//! Dart AOT snapshot loader — OSCortex strategic Path 2.
//!
//! Loads a Flutter AOT shared object (`app.aot`, an ELF64/ET_DYN) via the
//! kernel's `SYS_AOT_SNAPSHOT_LOAD` syscall (which maps the file PROT_R|X
//! into the calling process), then walks the ELF symbol table to resolve
//! the four Dart snapshot symbols:
//!
//!   * `_kDartVmSnapshotData`
//!   * `_kDartVmSnapshotInstructions`
//!   * `_kDartIsolateSnapshotData`
//!   * `_kDartIsolateSnapshotInstructions`
//!
//! These pointers are then written into the legacy `vm_snapshot_*` /
//! `isolate_snapshot_*` fields of `FlutterProjectArgs`, bypassing the
//! engine-internal `FlutterEngineCreateAOTData` → `Dart_LoadELF` path,
//! which assumes a host libc with `fopen`/`mmap` and crashes inside our
//! no-libc environment.
//!
//! This module is deliberately a thin, runtime-agnostic blob parser: it
//! knows ELF, it does **not** know Flutter beyond the four symbol names.
//! The same shape generalises to any future runtime that ships a Dart-
//! style AOT shared object (Unreal, Unity, Bevy, custom engines).

use crate::sys::{aot_snapshot_load, write};

// ── ELF64 constants ─────────────────────────────────────────────────────────

const EI_MAG0: usize = 0;
const EI_CLASS: usize = 4;
const EI_DATA: usize = 5;
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;

const PT_LOAD: u32 = 1;

const SHT_SYMTAB: u32 = 2;
const SHT_STRTAB: u32 = 3;
const SHT_DYNSYM: u32 = 11;

// ── Public surface ──────────────────────────────────────────────────────────

/// Resolved Dart snapshot pointers (4× ptr, 4× size).
#[derive(Default, Clone, Copy)]
pub struct DartSnapshotPointers {
    pub vm_data: u64,
    pub vm_data_size: u64,
    pub vm_instr: u64,
    pub vm_instr_size: u64,
    pub iso_data: u64,
    pub iso_data_size: u64,
    pub iso_instr: u64,
    pub iso_instr_size: u64,
}

impl DartSnapshotPointers {
    pub fn is_complete(&self) -> bool {
        self.vm_data != 0 && self.vm_instr != 0 && self.iso_data != 0 && self.iso_instr != 0
    }
}

/// Load a Dart AOT snapshot ELF from `path` and resolve all four snapshot
/// symbols. Returns `None` if the file is missing, not a valid ELF, or any
/// required symbol is absent.
///
/// On success the snapshot file is left mapped in the calling process at
/// the VA returned by `SYS_AOT_SNAPSHOT_LOAD`; the returned pointers are
/// stable for the lifetime of the process.
pub fn load_dart_snapshot(path: &[u8]) -> Option<(DartSnapshotPointers, u64)> {
    let mut aot_va: u64 = 0;
    let mut aot_size: u64 = 0;
    let rc = aot_snapshot_load(path, &mut aot_va, &mut aot_size);
    debug_hex(b"[embedder/aot] syscall rc=", rc as u64);
    debug_hex(b"[embedder/aot] syscall va=", aot_va);
    debug_hex(b"[embedder/aot] syscall sz=", aot_size);
    if rc != 0 || aot_va == 0 || aot_size < 64 {
        write(b"[embedder/aot] FAIL: syscall returned bad va/size\n");
        return None;
    }

    parse_dart_snapshot_elf(aot_va, aot_size as usize)
}

/// Parse an AOT ELF already mapped at `aot_va` (used by per-app host processes).
pub fn load_dart_snapshot_from_mapping(aot_va: u64) -> Option<(DartSnapshotPointers, u64)> {
    if aot_va == 0 {
        return None;
    }
    let span = infer_elf_file_span(aot_va)?;
    parse_dart_snapshot_elf(aot_va, span)
}

pub fn infer_elf_file_span(aot_va: u64) -> Option<usize> {
    let base = aot_va as *const u8;
    // SAFETY: reading ELF header and program headers from a kernel-mapped RX region.
    unsafe {
        let hdr = core::slice::from_raw_parts(base, 64);
        if hdr.len() < 64 || &hdr[..4] != b"\x7fELF" {
            return None;
        }
        let e_phoff = read_u64(hdr, 0x20)?;
        let e_phentsize = read_u16(hdr, 0x36)? as usize;
        let e_phnum = read_u16(hdr, 0x38)? as usize;
        let mut end = 64usize;
        for i in 0..e_phnum {
            let off = e_phoff as usize + i * e_phentsize;
            let ph = core::slice::from_raw_parts(base.add(off), 56);
            if ph.len() < 56 {
                return None;
            }
            let p_type = read_u32(ph, 0)?;
            if p_type != PT_LOAD {
                continue;
            }
            let p_offset = read_u64(ph, 8)? as usize;
            let p_filesz = read_u64(ph, 32)? as usize;
            end = end.max(p_offset.saturating_add(p_filesz));
        }
        Some(end)
    }
}

fn parse_dart_snapshot_elf(aot_va: u64, size: usize) -> Option<(DartSnapshotPointers, u64)> {
    let base = aot_va as *const u8;
    let buf = unsafe { core::slice::from_raw_parts(base, size) };

    // ── ELF64 header validation ──────────────────────────────────────────
    if buf.len() < 64 || &buf[..4] != b"\x7fELF" {
        return None;
    }
    if buf[EI_CLASS] != ELFCLASS64 || buf[EI_DATA] != ELFDATA2LSB {
        return None;
    }

    let e_phoff = read_u64(buf, 0x20)?;
    let e_shoff = read_u64(buf, 0x28)?;
    let e_phentsize = read_u16(buf, 0x36)? as usize;
    let e_phnum = read_u16(buf, 0x38)? as usize;
    let e_shentsize = read_u16(buf, 0x3A)? as usize;
    let e_shnum = read_u16(buf, 0x3C)? as usize;

    if e_phentsize < 56 || e_shentsize < 64 {
        return None;
    }

    // ── Collect PT_LOAD segments for VA→file-offset translation. ─────────
    // We use a tiny fixed-size table; real Dart AOTs ship 1-2 PT_LOAD entries.
    let mut loads: [LoadSeg; 8] = [LoadSeg::default(); 8];
    let mut nloads = 0usize;
    for i in 0..e_phnum {
        let off = (e_phoff as usize).checked_add(i * e_phentsize)?;
        if off + 56 > buf.len() {
            return None;
        }
        let p_type = read_u32(buf, off)?;
        if p_type != PT_LOAD {
            continue;
        }
        if nloads >= loads.len() {
            break;
        }
        loads[nloads] = LoadSeg {
            p_offset: read_u64(buf, off + 0x08)?,
            p_vaddr: read_u64(buf, off + 0x10)?,
            p_filesz: read_u64(buf, off + 0x20)?,
        };
        nloads += 1;
    }
    if nloads == 0 {
        return None;
    }

    // ── Walk section headers; find (sym, strtab) pair. ───────────────────
    // Prefer SHT_SYMTAB (full table) but fall back to SHT_DYNSYM (which
    // every Dart AOT ELF carries since the snapshot symbols are exported).
    let mut sym_off: u64 = 0;
    let mut sym_size: u64 = 0;
    let mut sym_entsz: u64 = 0;
    let mut sym_link: u32 = 0;
    let mut dyn_off: u64 = 0;
    let mut dyn_size: u64 = 0;
    let mut dyn_entsz: u64 = 0;
    let mut dyn_link: u32 = 0;
    for i in 0..e_shnum {
        let off = (e_shoff as usize).checked_add(i * e_shentsize)?;
        if off + 64 > buf.len() {
            return None;
        }
        let sh_type = read_u32(buf, off + 0x04)?;
        let sh_link = read_u32(buf, off + 0x28)?;
        let sh_offset = read_u64(buf, off + 0x18)?;
        let sh_size = read_u64(buf, off + 0x20)?;
        let sh_entsize = read_u64(buf, off + 0x38)?;
        match sh_type {
            SHT_SYMTAB => {
                sym_off = sh_offset;
                sym_size = sh_size;
                sym_entsz = sh_entsize;
                sym_link = sh_link;
            }
            SHT_DYNSYM => {
                dyn_off = sh_offset;
                dyn_size = sh_size;
                dyn_entsz = sh_entsize;
                dyn_link = sh_link;
            }
            _ => {}
        }
    }

    let (sym_o, sym_sz, sym_es, str_link) = if sym_off != 0 && sym_entsz >= 24 {
        (sym_off, sym_size, sym_entsz, sym_link)
    } else if dyn_off != 0 && dyn_entsz >= 24 {
        (dyn_off, dyn_size, dyn_entsz, dyn_link)
    } else {
        return None;
    };

    // Resolve associated string table.
    if (str_link as usize) >= e_shnum {
        return None;
    }
    let strtab_hdr = (e_shoff as usize).checked_add((str_link as usize) * e_shentsize)?;
    if strtab_hdr + 64 > buf.len() {
        return None;
    }
    let strtab_off = read_u64(buf, strtab_hdr + 0x18)? as usize;
    let strtab_size = read_u64(buf, strtab_hdr + 0x20)? as usize;
    if strtab_off.checked_add(strtab_size)? > buf.len() {
        return None;
    }
    let strtab = &buf[strtab_off..strtab_off + strtab_size];

    // ── Walk symbol table. ───────────────────────────────────────────────
    let mut out = DartSnapshotPointers::default();
    let count = (sym_sz / sym_es) as usize;
    let sym_base = sym_o as usize;
    for i in 0..count {
        let so = sym_base.checked_add(i * sym_es as usize)?;
        if so + 24 > buf.len() {
            return None;
        }
        let st_name = read_u32(buf, so + 0x00)?;
        let st_value = read_u64(buf, so + 0x08)?;
        let st_size = read_u64(buf, so + 0x10)?;
        if st_name == 0 {
            continue;
        }
        let name = cstr_at(strtab, st_name as usize)?;

        // Match against the four well-known names. Compare byte slices
        // directly — no allocator, no String.
        let (ptr_field, size_field): (&mut u64, &mut u64) = match name {
            b"_kDartVmSnapshotData" => (&mut out.vm_data, &mut out.vm_data_size),
            b"_kDartVmSnapshotInstructions" => (&mut out.vm_instr, &mut out.vm_instr_size),
            b"_kDartIsolateSnapshotData" => (&mut out.iso_data, &mut out.iso_data_size),
            b"_kDartIsolateSnapshotInstructions" => (&mut out.iso_instr, &mut out.iso_instr_size),
            _ => continue,
        };

        // VA→file-offset translation via the PT_LOAD that contains st_value.
        let abs = vaddr_to_abs(aot_va, &loads[..nloads], st_value)?;
        *ptr_field = abs;
        *size_field = st_size;
    }

    if !out.is_complete() {
        write(b"[embedder/aot] FAIL: symbol resolution incomplete\n");
        debug_hex(b"[embedder/aot]   vm_data=", out.vm_data);
        debug_hex(b"[embedder/aot]   vm_instr=", out.vm_instr);
        debug_hex(b"[embedder/aot]   iso_data=", out.iso_data);
        debug_hex(b"[embedder/aot]   iso_instr=", out.iso_instr);
        return None;
    }
    Some((out, aot_va))
}

// ── Helpers ─────────────────────────────────────────────────────────────────

#[derive(Default, Clone, Copy)]
struct LoadSeg {
    p_offset: u64,
    p_vaddr: u64,
    p_filesz: u64,
}

fn vaddr_to_abs(base: u64, loads: &[LoadSeg], vaddr: u64) -> Option<u64> {
    for l in loads {
        if vaddr >= l.p_vaddr && vaddr < l.p_vaddr.checked_add(l.p_filesz)? {
            let off_in_seg = vaddr - l.p_vaddr;
            let file_off = l.p_offset.checked_add(off_in_seg)?;
            return base.checked_add(file_off);
        }
    }
    None
}

fn cstr_at(strtab: &[u8], off: usize) -> Option<&[u8]> {
    if off >= strtab.len() {
        return None;
    }
    let mut end = off;
    while end < strtab.len() && strtab[end] != 0 {
        end += 1;
    }
    Some(&strtab[off..end])
}

#[inline]
fn read_u16(buf: &[u8], off: usize) -> Option<u16> {
    let s = buf.get(off..off + 2)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

#[inline]
fn read_u32(buf: &[u8], off: usize) -> Option<u32> {
    let s = buf.get(off..off + 4)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

#[inline]
fn read_u64(buf: &[u8], off: usize) -> Option<u64> {
    let s = buf.get(off..off + 8)?;
    Some(u64::from_le_bytes([
        s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
    ]))
}

// ── Logging helper ──────────────────────────────────────────────────────────
fn debug_hex(label: &[u8], val: u64) {
    let mut line = [0u8; 80];
    let mut n = 0;
    for &b in label {
        if n < line.len() {
            line[n] = b;
            n += 1;
        }
    }
    n = write_hex64(&mut line, n, b"0x", val);
    if n < line.len() {
        line[n] = b'\n';
        n += 1;
    }
    write(&line[..n]);
}

/// Pretty-print a resolved snapshot manifest to the serial log so we can
/// confirm what the loader bound.
pub fn log_manifest(p: &DartSnapshotPointers) {
    write(b"[embedder/aot] resolved Dart snapshot manifest:\n");
    log_pair(b"  vm_data       ", p.vm_data, p.vm_data_size);
    log_pair(b"  vm_instr      ", p.vm_instr, p.vm_instr_size);
    log_pair(b"  iso_data      ", p.iso_data, p.iso_data_size);
    log_pair(b"  iso_instr     ", p.iso_instr, p.iso_instr_size);
}

fn log_pair(label: &[u8], ptr: u64, size: u64) {
    let mut line = [0u8; 96];
    let mut n = 0;
    for &b in label {
        if n < line.len() {
            line[n] = b;
            n += 1;
        }
    }
    n = write_hex64(&mut line, n, b"@ 0x", ptr);
    n = write_hex64(&mut line, n, b"  size=0x", size);
    if n < line.len() {
        line[n] = b'\n';
        n += 1;
    }
    write(&line[..n]);
}

fn write_hex64(buf: &mut [u8], mut n: usize, prefix: &[u8], val: u64) -> usize {
    for &b in prefix {
        if n < buf.len() {
            buf[n] = b;
            n += 1;
        }
    }
    let digits = b"0123456789abcdef";
    for i in 0..16 {
        if n >= buf.len() {
            break;
        }
        let nyb = ((val >> ((15 - i) * 4)) & 0xF) as usize;
        buf[n] = digits[nyb];
        n += 1;
    }
    n
}
