
//! Code that lives in -- and runs straight out of -- the external SPI NOR.
//!
//! The SFC maps the W25Q128 at `0x7000_0000`, and the core fetches instructions
//! from that window like from any other memory. `memory.x` parks everything
//! marked `.sfc_xip*` there, and `tools/flash_xip.sh` copies that section out
//! of the ELF and programs it into the flash at the matching offset.
//!
//! Two rules for anything placed there:
//!
//! * **No calls out.** `bl` reaches +/-16 MB and the internal flash sits 1.8 GB
//!   away, so callers use the function pointers in [`XIP_TABLE`] (an absolute
//!   address, loaded with `movw`/`movt`) instead. A XIP function must therefore
//!   be self-contained: no panics, no slicing, no helpers from other crates.
//! * **Nothing there runs before the SFC is up.** Reading the window while the
//!   controller is unconfigured faults, so [`available`] is only ever called
//!   after `Sfc::new`, and it verifies the image (magic + one trial run) before
//!   anything else trusts it. When the flash is blank or the image is stale,
//!   the demo falls back to the copy of the code in internal flash.

use core::ptr;
use rtt_target::rprintln;

/// "XIPX": first word of the image, checked before anything in it is called.
pub const MAGIC: u32 = 0x5849_5058;
/// Bump when [`XipTable`] changes; a mismatch means "do not use".
const VERSION: u32 = 1;

/// The one absolute address internal-flash code needs.
///
/// It sits at the very start of the XIP image, so a caller reads the table once
/// and reaches everything else in the image through it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct XipTable {
    pub magic: u32,
    pub version: u32,
    /// Expand `pixels` big-endian RGB565 words into RGB888 triples.
    pub rgb565_to_rgb888: unsafe extern "C" fn(dst: *mut u8, src: *const u8, pixels: usize),
}

/// Placed at `SFC_XIP_BASE` by `memory.x`.
#[no_mangle]
#[used]
#[link_section = ".sfc_xip.table"]
pub static XIP_TABLE: XipTable = XipTable {
    magic: MAGIC,
    version: VERSION,
    rgb565_to_rgb888: rgb565_to_rgb888,
};

/// Expand `pixels` big-endian RGB565 words from `src` into RGB888 at `dst`.
///
/// Deliberately written with raw pointers: no indexing (which panics), no
/// helpers, nothing that could turn into a branch back to internal flash.
// Kept because `XIP_TABLE` points at it; `#[no_mangle]` gives it a stable name
// so the image can be disassembled by symbol.
#[no_mangle]
#[link_section = ".sfc_xip.text"]
unsafe extern "C" fn rgb565_to_rgb888(dst: *mut u8, src: *const u8, pixels: usize) {
    let mut i = 0;
    while i < pixels {
        let hi = ptr::read(src.add(i * 2)) as u16;
        let lo = ptr::read(src.add(i * 2 + 1)) as u16;
        let word = (hi << 8) | lo;

        let r5 = ((word >> 11) & 0x1F) as u8;
        let g6 = ((word >> 5) & 0x3F) as u8;
        let b5 = (word & 0x1F) as u8;

        ptr::write(dst.add(i * 3), (r5 << 3) | (r5 >> 2));
        ptr::write(dst.add(i * 3 + 1), (g6 << 2) | (g6 >> 4));
        ptr::write(dst.add(i * 3 + 2), (b5 << 3) | (b5 >> 2));

        i += 1;
    }
}

/// Read the table out of the flash window.
///
/// Only valid once the SFC has been configured: before that the window is not
/// mapped and touching it faults.
pub fn table() -> XipTable {
    unsafe { ptr::read_volatile(ptr::addr_of!(XIP_TABLE)) }
}

/// True when the window really holds a matching, working image.
///
/// Checks the magic, the version, and then runs the function once on a known
/// input -- a blank or stale flash (or a wrong width/quad setting) is reported
/// here instead of as a hard fault inside the gallery.
pub fn available() -> bool {
    let table = table();
    if table.magic != MAGIC || table.version != VERSION {
        // Says whether the window is mapped at all: garbage here means the image
        // was never programmed (or the SFC is misconfigured), not that the code
        // in it is wrong.
        rprintln!(
            "xip: no usable image (magic {:#010X}, version {})",
            table.magic,
            table.version
        );
        return false;
    }

    // 0xF81F is magenta in RGB565: red and blue at full scale, green at zero.
    // The green channel is what makes this a real check -- a run that produced
    // nothing leaves the buffer zeroed, and a wrong scaling shows up there too.
    let src = [0xF8u8, 0x1F];
    let mut dst = [0u8; 3];
    unsafe { (table.rgb565_to_rgb888)(dst.as_mut_ptr(), src.as_ptr(), 1) };
    if dst != [0xFF, 0x00, 0xFF] {
        // The image is there and the code ran, so this is the interesting
        // failure: instruction fetch through the window returning something
        // other than what is in the flash.
        rprintln!(
            "xip: trial run returned {:#04X} {:#04X} {:#04X}, want FF 00 FF",
            dst[0],
            dst[1],
            dst[2]
        );
        return false;
    }
    true
}
