/* OSCortex x86_64 linker script — Limine higher-half kernel */
/* Kernel is loaded at -2 GiB (0xFFFFFFFF80000000) */

OUTPUT_FORMAT("elf64-x86-64")
OUTPUT_ARCH(i386:x86-64)
ENTRY(kernel_main)

PHDRS {
    text    PT_LOAD FLAGS(5);  /* R-X */
    rodata  PT_LOAD FLAGS(4);  /* R-- */
    data    PT_LOAD FLAGS(6);  /* RW- */
    dynamic PT_DYNAMIC;
}

SECTIONS {
    . = 0xFFFFFFFF80000000;

    /* ── Text ─────────────────────────────────────────────────────────── */
    .text : {
        *(.text .text.*)
    } :text

    . = ALIGN(4096);

    /* ── Read-only data ───────────────────────────────────────────────── */
    .rodata : {
        *(.rodata .rodata.*)
    } :rodata

    /* Limine requests (must be in rodata) */
    .limine_reqs : {
        KEEP(*(.limine_requests*))
        KEEP(*(.limine_requests_start*))
        KEEP(*(.limine_requests_end*))
    } :rodata

    . = ALIGN(4096);

    /* ── Data ─────────────────────────────────────────────────────────── */
    .data : {
        *(.data .data.*)
    } :data

    /* Cortex Arena — isolated RW region for AI runtime (4 MiB reserved) */
    . = ALIGN(4096);
    __cortex_arena_start = .;
    .cortex_arena (NOLOAD) : {
        . += 4 * 1024 * 1024;
    } :data
    __cortex_arena_end = .;

    . = ALIGN(4096);
    .bss : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    } :data

    . = ALIGN(4096);

    /* ── Discard ──────────────────────────────────────────────────────── */
    /DISCARD/ : {
        *(.eh_frame)
        *(.note .note.*)
    }
}
