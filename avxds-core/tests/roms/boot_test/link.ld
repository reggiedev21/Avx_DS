/* LOAD_ADDR est passé par l'outil de build (--defsym=LOAD_ADDR=…). */
ENTRY(_start)
SECTIONS
{
    . = LOAD_ADDR;
    .text : { KEEP(*(.text.start)) *(.text .text.*) }
    .rodata : { *(.rodata .rodata.*) }
    .data : { *(.data .data.*) }
    . = ALIGN(4);
    __bss_start = .;
    .bss : { *(.bss .bss.* COMMON) }
    . = ALIGN(4);
    __bss_end = .;
    /DISCARD/ : { *(.ARM.exidx*) *(.ARM.extab*) *(.comment) }
}
