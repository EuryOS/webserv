/*
 * EuryOS SDK 0.1.0 native package layout.
 *
 * Package binaries are loaded into their own AddressSpace. The fixed low VA
 * and bounded stack are part of the package-loader contract for this bundle.
 */
ENTRY(_start)

SECTIONS
{
    . = 0x40000000;

    .text : ALIGN(4096) { *(.text .text.*) }
    .rodata : ALIGN(4096) { *(.rodata .rodata.*) }
    .data : ALIGN(4096) { *(.data .data.*) }

    .bss (NOLOAD) : ALIGN(4096)
    {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)

        /* The loader maps and zeroes this 64 KiB stack with .bss. */
        . = ALIGN(4096);
        __stack_bottom = .;
        . = . + 0x10000;
        __stack_top = .;

        __bss_end = .;
    }

    . = ALIGN(16);

    /DISCARD/ :
    {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
    }
}
