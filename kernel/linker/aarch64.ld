/* aarch64 linker script stub */
OUTPUT_FORMAT("elf64-littleaarch64")
ENTRY(kernel_main)
SECTIONS {
    . = 0xFFFF000000080000;
    .text : { *(.text .text.*) }
    . = ALIGN(4096);
    .rodata : { *(.rodata .rodata.*) }
    . = ALIGN(4096);
    .data : { *(.data .data.*) }
    .bss : { __bss_start = .; *(.bss .bss.*) *(COMMON) __bss_end = .; }
    /DISCARD/ : { *(.eh_frame) *(.note .note.*) }
}
