/* SWM34S memory layout: 512K flash @ 0x0, 64K SRAM @ 0x2000_0000.
 *
 * The 8 MB SDRAM window at 0x8000_0000 is deliberately *not* declared here: it
 * is claimed at runtime by this application (framebuffers + heap) and must stay
 * out of the linker's reach.
 *
 * The SPI NOR (W25Q128) is mapped by the SFC at 0x7000_0000, and the core can
 * take instructions straight out of that window -- XiP. The region below is
 * where the linker puts everything marked `.sfc_xip*`; `tools/flash_xip.sh`
 * cuts that section out of the ELF and programs it into the flash at the
 * matching offset (2 MiB in), so the two addresses must agree:
 *
 *     SFC_XIP_BASE == hal::sfc::FLASH_BASE + flash offset of the XIP image
 */
PROVIDE(FLASH_BASE = 0x00000000);
PROVIDE(FLASH_SIZE = 512K);
PROVIDE(RAM_BASE = 0x20000000);
PROVIDE(RAM_SIZE = 64K);

PROVIDE(SFC_XIP_BASE = 0x70200000);
/* Room for the gallery (each 480x320 RGB565 picture costs 300 KB) plus the code
 * that runs from there. The NOR is 16 MB and the window covers all of it, so
 * this is only a sanity bound: raise it when the linker reports an overflow. */
PROVIDE(SFC_XIP_SIZE = 4M);

MEMORY
{
  FLASH   : ORIGIN = FLASH_BASE,   LENGTH = FLASH_SIZE
  RAM     : ORIGIN = RAM_BASE,     LENGTH = RAM_SIZE
  SFC_XIP : ORIGIN = SFC_XIP_BASE, LENGTH = SFC_XIP_SIZE
}

/* The image is laid out table-first: the table holds the entry points of
 * everything else in it, so code in internal flash needs exactly one absolute
 * address (XIP_TABLE) and no branch across the 1.8 GB between the two windows
 * (a `bl` only reaches +/-16 MB, which is why XIP functions are always called
 * through the function pointers in this table). */
SECTIONS
{
  .sfc_xip : ALIGN(4)
  {
    __sfc_xip_start = .;
    KEEP(*(.sfc_xip.table))
    KEEP(*(.sfc_xip.text .sfc_xip.text.*))
    KEEP(*(.sfc_xip.rodata .sfc_xip.rodata.*))
    /* the pictures, as raw RGB565 */
    KEEP(*(.sfc_xip.gallery))
    KEEP(*(.sfc_xip .sfc_xip.*))
    . = ALIGN(4);
    __sfc_xip_end = .;
  } > SFC_XIP

  __sfc_xip_size = __sfc_xip_end - __sfc_xip_start;
}
