/* tools/flutter-embedder/user_aarch64.ld
 * Userspace ELF linker script for OSCortex — aarch64.
 * Load address: USER_ELF_BASE = 0x400000 (4 MiB), matching the kernel's
 * process loader expectation (shared across architectures).
 */

OUTPUT_FORMAT("elf64-littleaarch64")
OUTPUT_ARCH(aarch64)
ENTRY(_start)

SECTIONS {
    . = 0x400000;

    .text : {
        KEEP(*(.text._start))
        *(.text .text.*)
        *(.plt*)
    }

    .rodata : {
        *(.rodata .rodata.*)
    }

    . = ALIGN(4096);

    .data : {
        *(.data .data.*)
        *(.got*)
    }

    .bss : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    /DISCARD/ : {
        *(.comment*)
        *(.note*)
        *(.eh_frame*)
        *(.gnu.hash)
        *(.dynsym)
        *(.dynstr)
        *(.dynamic)
        *(.rel*)
        *(.rela*)
    }
}
