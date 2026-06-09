//! ELF64 loader — parses a statically-linked ELF and maps PT_LOAD segments
//! into a process's user address space.

use crate::mm::{frame_allocator, paging};

// ── ELF64 constants ───────────────────────────────────────────────────────────

const ELF_MAGIC:      [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELFCLASS64:     u8 = 2;
const ELFDATA2LSB:    u8 = 1; // little-endian
const ET_EXEC:        u16 = 2;
const ET_DYN:         u16 = 3; // PIE
const EM_X86_64:      u16 = 62;
const EM_AARCH64:     u16 = 183;
const PT_LOAD:        u32 = 1;

/// The ELF machine type this kernel build can execute natively.
#[cfg(target_arch = "x86_64")]
const EM_NATIVE: u16 = EM_X86_64;
#[cfg(target_arch = "aarch64")]
const EM_NATIVE: u16 = EM_AARCH64;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const EM_NATIVE: u16 = EM_X86_64;
const PF_X:           u32 = 1 << 0; // executable
const PF_W:           u32 = 1 << 1; // writable

// ── ELF64 structures ──────────────────────────────────────────────────────────

/// ELF64 file header (64 bytes).
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

/// ELF64 program header (56 bytes).
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

// ── Helper: read a packed value safely ───────────────────────────────────────

/// Copy `size_of::<T>()` bytes from `bytes[offset..]` into a T.
/// Returns None if the slice is too short.
fn read_struct<T: Copy>(bytes: &[u8], offset: usize) -> Option<T> {
    let size = core::mem::size_of::<T>();
    if offset + size > bytes.len() { return None; }
    // SAFETY: we just checked bounds; T is repr(C, packed) so no alignment req.
    Some(unsafe {
        core::ptr::read_unaligned(bytes.as_ptr().add(offset) as *const T)
    })
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Load an ELF64 binary into the address space described by `pml4_phys`.
///
/// Returns the virtual entry point on success.
pub fn load(bytes: &[u8], pml4_phys: u64) -> Result<u64, &'static str> {
    // ── Validate header ───────────────────────────────────────────────────
    let hdr: Elf64Hdr = read_struct(bytes, 0).ok_or("ELF too short")?;

    if hdr.e_ident[0..4] != ELF_MAGIC           { return Err("bad ELF magic"); }
    if hdr.e_ident[4]    != ELFCLASS64           { return Err("not ELF64"); }
    if hdr.e_ident[5]    != ELFDATA2LSB          { return Err("not little-endian"); }
    if hdr.e_machine     != EM_NATIVE            { return Err("wrong ELF machine for arch"); }
    if hdr.e_type != ET_EXEC && hdr.e_type != ET_DYN {
        return Err("not executable ELF");
    }
    if hdr.e_phentsize as usize != core::mem::size_of::<Elf64Phdr>() {
        return Err("unexpected phentsize");
    }

    let entry = hdr.e_entry;
    let ph_off  = hdr.e_phoff  as usize;
    let ph_num  = hdr.e_phnum  as usize;
    let ph_size = hdr.e_phentsize as usize;

    // ── Walk PT_LOAD segments ─────────────────────────────────────────────
    for i in 0..ph_num {
        let ph_base = ph_off + i * ph_size;
        let ph: Elf64Phdr = read_struct(bytes, ph_base).ok_or("pheader OOB")?;
        if ph.p_type != PT_LOAD { continue; }
        if ph.p_memsz == 0 { continue; }

        let vaddr  = ph.p_vaddr;
        let memsz  = ph.p_memsz  as usize;
        let filesz = ph.p_filesz as usize;
        let offset = ph.p_offset as usize;
        let writable = (ph.p_flags & PF_W) != 0;
        let exec     = (ph.p_flags & PF_X) != 0;

        // Round vaddr down to page boundary.
        let page_start = vaddr & !0xFFF;
        let page_off   = (vaddr - page_start) as usize;
        let total_bytes = page_off + memsz;
        let num_pages = total_bytes.div_ceil(4096);

        for p in 0..num_pages {
            let page_virt  = page_start + (p * 4096) as u64;

            // Check whether this virtual page was already mapped by a previous
            // PT_LOAD segment (common when code and rodata share a 4 KiB page).
            // If so, reuse the existing physical frame so we don't lose the
            // bytes written by the earlier segment, and preserve the more-
            // permissive (exec) permission that the earlier segment set.
            let existing_phys = crate::mm::paging::translate_user_page(pml4_phys, page_virt);
            let (page_phys, is_new_frame) = if let Some(phys) = existing_phys {
                (phys, false)
            } else {
                let phys = frame_allocator::alloc_frame().ok_or("OOM: elf segment")?;
                (phys, true)
            };

            let page_vaddr = (page_phys + frame_allocator::hhdm_offset()) as *mut u8;

            // Zero only freshly-allocated pages.
            if is_new_frame {
                unsafe { core::ptr::write_bytes(page_vaddr, 0, 4096); }
            }

            // Copy file data into page.
            let byte_start = p * 4096;
            let byte_end   = (byte_start + 4096).min(total_bytes);
            for byte_idx in byte_start..byte_end {
                if byte_idx < page_off { continue; } // pre-alignment gap
                let seg_byte = byte_idx - page_off;
                if seg_byte < filesz {
                    let src_off = offset + seg_byte;
                    if src_off < bytes.len() {
                        unsafe {
                            *page_vaddr.add(byte_idx - byte_start) = bytes[src_off];
                        }
                    }
                }
                // bytes beyond filesz stay zeroed (BSS) for new frames;
                // for existing frames only the segment's own bytes are written.
            }

            // Map (or re-map) the page.  When re-mapping a shared page, use
            // the MORE permissive exec flag so that a code+rodata overlap page
            // remains executable even though rodata has PF_X clear.
            let map_exec = exec || existing_phys.is_some();
            unsafe {
                paging::map_user_page_with_flags(
                    pml4_phys,
                    page_virt,
                    page_phys,
                    writable,
                    map_exec,
                )?;
            }
        }

        log::debug!("[ELF] PT_LOAD vaddr={:#x} memsz={} filesz={} {} {}",
            vaddr, memsz, filesz,
            if writable { "rw" } else { "ro" },
            if exec { "exec" } else { "" },
        );
    }

    log::info!("[ELF] Loaded: entry={:#x}", entry);
    Ok(entry)
}
