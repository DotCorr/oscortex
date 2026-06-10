/* aarch64 direct-boot linker script (QEMU -M virt -kernel).
 *
 * QEMU `-M virt` places RAM at 0x4000_0000 and, for an ELF kernel, loads each
 * PT_LOAD segment at its physical address and jumps to e_entry with the MMU
 * OFF (running on physical == virtual identity at this point). We therefore
 * link the kernel to run from a low physical address and enable the MMU later
 * with an identity map plus the kernel's higher-half window.
 *
 * Entry is the assembly `_start` (boot.rs) which sets up the stack, zeroes BSS,
 * brings up the PL011 UART for serial, then calls into the Rust bring-up path.
 */
OUTPUT_FORMAT("elf64-littleaarch64")
OUTPUT_ARCH(aarch64)
ENTRY(_start)

/* QEMU -M virt RAM base. The kernel is loaded here by -kernel. */
KERNEL_PHYS_BASE = 0x40080000;

SECTIONS {
    . = KERNEL_PHYS_BASE;

    .text : ALIGN(4096) {
        __text_start = .;
        KEEP(*(.text.boot))   /* _start must be first */
        *(.text .text.*)
        __text_end = .;
    }

    .rodata : ALIGN(4096) {
        __rodata_start = .;
        *(.rodata .rodata.*)
        __rodata_end = .;
    }

    .data : ALIGN(4096) {
        __data_start = .;
        *(.data .data.*)
        *(.got .got.*)
        __data_end = .;
    }

    .bss (NOLOAD) : ALIGN(4096) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }

    . = ALIGN(4096);
    __kernel_end = .;

    /DISCARD/ : { *(.eh_frame) *(.eh_frame_hdr) *(.note .note.*) *(.comment) }
}
