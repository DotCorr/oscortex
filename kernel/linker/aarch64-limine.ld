/* OSCortex aarch64 linker script — Limine higher-half kernel (UEFI boot).
 *
 * Unlike the bare `-kernel` path (aarch64.ld, linked to physical 0x40080000 with
 * the MMU off and entry `_start`), the Limine boot protocol hands the kernel a
 * machine that already has the MMU on, a higher-half mapping of the kernel, an
 * HHDM direct-map of physical RAM, and a framebuffer. Limine loads each PT_LOAD
 * segment at its higher-half virtual link address and jumps to the ELF entry
 * point at EL1.
 *
 * We therefore link this variant to the conventional Limine higher-half base
 * (-2 GiB) and set the entry to the aarch64 Limine entry trampoline, mirroring
 * the x86_64 Limine kernel layout. The Limine request statics are discovered by
 * the bootloader by scanning the loaded image for their magic numbers.
 */
OUTPUT_FORMAT("elf64-littleaarch64")
OUTPUT_ARCH(aarch64)
ENTRY(_limine_aarch64_entry)

PHDRS {
    text    PT_LOAD FLAGS(5);  /* R-X */
    rodata  PT_LOAD FLAGS(4);  /* R-- */
    data    PT_LOAD FLAGS(6);  /* RW- */
}

SECTIONS {
    . = 0xFFFFFFFF80000000;

    .text : ALIGN(4096) {
        __text_start = .;
        KEEP(*(.text.limine_entry))   /* entry trampoline first */
        *(.text .text.*)
        __text_end = .;
    } :text

    . = ALIGN(4096);

    .rodata : ALIGN(4096) {
        __rodata_start = .;
        *(.rodata .rodata.*)
        __rodata_end = .;
    } :rodata

    /* Limine requests (scanned by the bootloader for their magic numbers). */
    .limine_reqs : {
        KEEP(*(.limine_requests*))
        KEEP(*(.limine_requests_start*))
        KEEP(*(.limine_requests_end*))
    } :rodata

    . = ALIGN(4096);

    .data : ALIGN(4096) {
        __data_start = .;
        *(.data .data.*)
        *(.got .got.*)
        __data_end = .;
    } :data

    . = ALIGN(4096);
    .bss (NOLOAD) : ALIGN(4096) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    } :data

    . = ALIGN(4096);
    __kernel_end = .;

    /DISCARD/ : { *(.eh_frame) *(.eh_frame_hdr) *(.note .note.*) *(.comment) }
}
