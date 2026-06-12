//! Minimal flattened-device-tree (FDT / DTB) reader for the aarch64 port.
//!
//! On `qemu-system-aarch64 -M virt` there is no Limine bootloader: QEMU loads
//! the kernel via `-kernel` and passes a pointer to a Flattened Device Tree blob
//! in `x0` at `_start`. The DTB describes the machine — RAM base/size, and the
//! MMIO layout of the UART, GIC, timer, etc.
//!
//! This is a deliberately small, hand-rolled, `no_std`/no-alloc parser (no
//! external crate). It understands just enough of the DTB v17 structure to:
//!   * validate the header magic,
//!   * walk the structure block,
//!   * extract the `/memory` node's `reg` (RAM base + size), and
//!   * read `#address-cells` / `#size-cells` so multi-cell `reg` values decode.
//!
//! Spec reference: the Devicetree Specification (flattened format). All
//! multi-byte integers in a DTB are **big-endian**.

use core::sync::atomic::{AtomicU64, Ordering};

// ── FDT header constants ────────────────────────────────────────────────────

const FDT_MAGIC: u32 = 0xd00d_feed;

const FDT_BEGIN_NODE: u32 = 0x0000_0001;
const FDT_END_NODE: u32 = 0x0000_0002;
const FDT_PROP: u32 = 0x0000_0003;
const FDT_NOP: u32 = 0x0000_0004;
const FDT_END: u32 = 0x0000_0009;

/// The DTB pointer QEMU passed in x0, captured at `_start`. Zero until set.
static DTB_PTR: AtomicU64 = AtomicU64::new(0);

/// Record the physical address of the DTB (called from the boot path with the
/// value `_start` saved out of x0).
pub fn set_dtb_ptr(pa: u64) {
    DTB_PTR.store(pa, Ordering::Release);
}

/// The recorded DTB pointer (0 if none was captured).
pub fn dtb_ptr() -> u64 {
    DTB_PTR.load(Ordering::Acquire)
}

// ── Big-endian readers over the raw blob ────────────────────────────────────

#[inline]
unsafe fn read_be32(p: *const u8) -> u32 {
    u32::from_be_bytes([*p, *p.add(1), *p.add(2), *p.add(3)])
}

#[inline]
unsafe fn read_be64(p: *const u8) -> u64 {
    let hi = read_be32(p) as u64;
    let lo = read_be32(p.add(4)) as u64;
    (hi << 32) | lo
}

// ── Header view ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct FdtHeader {
    base: *const u8,
    off_dt_struct: u32,
    off_dt_strings: u32,
    size_dt_struct: u32,
}

impl FdtHeader {
    /// Validate and parse the FDT header at `base`. Returns `None` if the magic
    /// is wrong (no/invalid DTB).
    unsafe fn parse(base: *const u8) -> Option<FdtHeader> {
        if base.is_null() {
            return None;
        }
        let magic = read_be32(base);
        if magic != FDT_MAGIC {
            return None;
        }
        Some(FdtHeader {
            base,
            // Layout: magic(0) totalsize(4) off_dt_struct(8) off_dt_strings(12)
            //         off_mem_rsvmap(16) version(20) ...
            //         size_dt_strings(32) size_dt_struct(36)
            off_dt_struct: read_be32(base.add(8)),
            off_dt_strings: read_be32(base.add(12)),
            size_dt_struct: read_be32(base.add(36)),
        })
    }

    #[inline]
    unsafe fn struct_start(&self) -> *const u8 {
        self.base.add(self.off_dt_struct as usize)
    }

    #[inline]
    unsafe fn struct_end(&self) -> *const u8 {
        self.base
            .add(self.off_dt_struct as usize + self.size_dt_struct as usize)
    }

    /// Resolve a property name by its offset into the strings block.
    unsafe fn string_at(&self, off: u32) -> &'static str {
        let p = self.base.add(self.off_dt_strings as usize + off as usize);
        let mut len = 0usize;
        while *p.add(len) != 0 {
            len += 1;
        }
        let bytes = core::slice::from_raw_parts(p, len);
        core::str::from_utf8(bytes).unwrap_or("")
    }
}

/// A single discovered memory region (physical base + length, in bytes).
#[derive(Clone, Copy, Debug)]
pub struct MemRegion {
    pub base: u64,
    pub size: u64,
}

/// Result of parsing the device tree for the bits the kernel needs.
#[derive(Clone, Copy)]
pub struct FdtInfo {
    /// Discovered RAM regions (from `/memory` nodes). Up to 4 retained.
    pub mem: [MemRegion; 4],
    pub mem_count: usize,
}

impl FdtInfo {
    const fn empty() -> Self {
        FdtInfo {
            mem: [MemRegion { base: 0, size: 0 }; 4],
            mem_count: 0,
        }
    }

    /// Total RAM bytes across all discovered regions.
    pub fn ram_total(&self) -> u64 {
        let mut t = 0u64;
        for i in 0..self.mem_count {
            t += self.mem[i].size;
        }
        t
    }
}

/// Walk the DTB structure block and extract the `/memory` node(s).
///
/// Returns `None` if no valid DTB is present at `dtb_pa`.
///
/// # Safety
/// `dtb_pa` must point at a DTB blob mapped/identity-accessible at this address.
pub unsafe fn parse(dtb_pa: u64) -> Option<FdtInfo> {
    let hdr = FdtHeader::parse(dtb_pa as *const u8)?;

    let mut info = FdtInfo::empty();

    // Root cell sizes default to 2/1 per spec; QEMU virt uses 2/2.
    let mut root_addr_cells: u32 = 2;
    let mut root_size_cells: u32 = 2;
    // Cell sizes that apply to the *current* node's children (set when we see
    // the property on the parent). We track only one level of nesting context
    // since /memory sits directly under the root.
    let mut cur_addr_cells: u32 = 2;
    let mut cur_size_cells: u32 = 2;

    let mut p = hdr.struct_start();
    let end = hdr.struct_end();

    // Depth tracking: depth 0 = root. /memory is depth 1.
    let mut depth: i32 = -1;
    // Whether the node we're currently inside is a `/memory*` node.
    let mut in_memory = false;

    while p < end {
        let token = read_be32(p);
        p = p.add(4);
        match token {
            t if t == FDT_BEGIN_NODE => {
                // Node name is a NUL-terminated string, padded to 4 bytes.
                let name_ptr = p;
                let mut nlen = 0usize;
                while *name_ptr.add(nlen) != 0 {
                    nlen += 1;
                }
                let name = core::str::from_utf8(core::slice::from_raw_parts(name_ptr, nlen))
                    .unwrap_or("");
                // Advance past name + NUL, aligned up to 4.
                p = p.add((nlen + 4) & !3);

                depth += 1;
                if depth == 1 {
                    // Entering a top-level node: inherit the root cell sizes as
                    // the defaults for this node's `reg`.
                    cur_addr_cells = root_addr_cells;
                    cur_size_cells = root_size_cells;
                    // A memory node is named "memory" or "memory@<addr>".
                    in_memory = name == "memory" || name.starts_with("memory@");
                }
            }
            t if t == FDT_END_NODE => {
                if depth == 1 {
                    in_memory = false;
                }
                depth -= 1;
            }
            t if t == FDT_PROP => {
                // prop header: len(4) nameoff(4), then `len` bytes of value,
                // padded to 4.
                let len = read_be32(p);
                let nameoff = read_be32(p.add(4));
                let val = p.add(8);
                let pname = hdr.string_at(nameoff);

                if depth == 0 {
                    // Root-level cell sizes.
                    if pname == "#address-cells" && len == 4 {
                        root_addr_cells = read_be32(val);
                    } else if pname == "#size-cells" && len == 4 {
                        root_size_cells = read_be32(val);
                    }
                } else if depth == 1 && in_memory && pname == "reg" {
                    // `reg` = a list of (address, size) pairs, each cell being a
                    // big-endian u32; address spans `cur_addr_cells` cells, size
                    // spans `cur_size_cells` cells.
                    let pair_cells = (cur_addr_cells + cur_size_cells) as usize;
                    let total_cells = (len as usize) / 4;
                    let mut c = 0usize;
                    while c + pair_cells <= total_cells {
                        let base = read_cells(val.add(c * 4), cur_addr_cells);
                        let size = read_cells(
                            val.add((c + cur_addr_cells as usize) * 4),
                            cur_size_cells,
                        );
                        if size != 0 && info.mem_count < info.mem.len() {
                            info.mem[info.mem_count] = MemRegion { base, size };
                            info.mem_count += 1;
                        }
                        c += pair_cells;
                    }
                }

                // Advance past the value (padded to 4).
                p = p.add(((len as usize) + 3) & !3);
            }
            t if t == FDT_NOP => { /* skip */ }
            t if t == FDT_END => break,
            _ => break, // malformed — stop rather than run off the blob
        }
    }

    if info.mem_count == 0 {
        None
    } else {
        Some(info)
    }
}

/// Scan a physical address range for the FDT magic on an 8-byte stride and
/// return the first address whose blob parses to a valid `/memory` node.
///
/// QEMU's `-M virt` ELF-kernel boot does not reliably pass the DTB pointer in x0
/// and may place the generated blob at a position that depends on the kernel
/// image size. As a robust last resort, we scan a bounded window of RAM for it.
///
/// # Safety
/// `[start, end)` must be readable (identity-mapped) physical RAM.
pub unsafe fn scan_for_dtb(start: u64, end: u64) -> Option<u64> {
    let mut p = start & !0x7;
    while p < end {
        if read_be32(p as *const u8) == FDT_MAGIC {
            if parse(p).is_some() {
                return Some(p);
            }
        }
        p += 8;
    }
    None
}

/// Read a 1- or 2-cell big-endian value (cells beyond 2 are ignored / unsupported).
#[inline]
unsafe fn read_cells(p: *const u8, cells: u32) -> u64 {
    match cells {
        1 => read_be32(p) as u64,
        2 => read_be64(p),
        _ => read_be64(p), // clamp: treat >2 as 2 (top 64 bits)
    }
}
