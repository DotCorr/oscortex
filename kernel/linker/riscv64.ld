/* OSCortex RISC-V 64 linker script — Limine higher-half kernel */
/* Kernel loaded at 0xFFFFFFFF80000000 (same higher-half convention) */

OUTPUT_ARCH(riscv)
ENTRY(kernel_main)

SECTIONS {
    . = 0xFFFFFFFF80000000;

    .text : {
        *(.text.init)
        *(.text .text.*)
    }

    . = ALIGN(4096);

    .rodata : {
        *(.rodata .rodata.*)
        /* Limine requests */
        KEEP(*(.limine_reqs))
    }

    . = ALIGN(4096);

    .data : {
        *(.data .data.*)
        *(.got .got.*)
    }

    . = ALIGN(4096);

    .bss : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    /DISCARD/ : {
        *(.eh_frame)
        *(.note .note.*)
    }
}
