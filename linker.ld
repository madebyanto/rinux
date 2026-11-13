ENTRY(_start)

SECTIONS
{
    . = 1M;

    __kernel_start = .;

    .text : ALIGN(4K)
    {
        KEEP(*(.multiboot2_header))
        *(.text .text.*)
    }

    .rodata : ALIGN(4K)
    {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(4K)
    {
        *(.data .data.*)
    }

    .bss : ALIGN(4K)
    {
        *(.bss .bss.*)
        *(COMMON)
    }

    __kernel_end = .;

    /DISCARD/ :
    {
        *(.eh_frame*)
        *(.comment)
        *(.note*)
    }
}
