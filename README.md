# swm341-slint

Slint GUI on the Synwit SWM341: an 800x480 RGB565 parallel TFT driven by the
LCDC, two framebuffers plus Slint's allocator in external SDRAM, a picture
gallery whose pixels and even some of the code live in the on-board SPI NOR
flash (XiP), and embassy driving the tasks that would otherwise stall the UI.

```
src/
  main.rs                bring-up (clocks, SDRAM, LCDC, SFC) + the Slint loop,
                         plus the embassy task that loads pictures
  assets.rs              picture table + chunked async loader for the XiP section
  platform.rs            slint::platform::Platform impl (window adapter + clock)
  time.rs                embassy_time (SysTick, installed by hal::init) -> Duration
  input.rs               where touch/button input goes (stub until a panel is wired up)
  xip.rs                 code that lives in (and runs from) the SPI NOR
ui/main.slint            the UI itself
tools/make_gallery.py    packs the artwork (incl. rasterised SVG) into the image
tools/run.sh             cargo runner: programs .sfc_xip into the SPI NOR, then runs
```

## Build and run

```sh
cargo run                 # one command: internal flash + SPI NOR + RTT attached
cargo build --release
```

`.cargo/config.toml` sets the target and the runner (`tools/run.sh`), so
`cargo run` is enough: the runner cuts the `.sfc_xip` section out of the ELF and
programs it into the SPI NOR with probe-rs' own `swm341_spiflash` algorithm
(only when it changed -- its hash is cached in `assets/build/`), then flashes
the firmware itself and attaches RTT.

Current footprint (debug): internal flash `279 KB .text + 199 KB .rodata`
(~478 KB of 512 KB), `bss` ~6.6 KB of 64 KB SRAM (the executor's task storage) --
and a **1.2 MB `.sfc_xip` section at `0x7020_0000`**, i.e. in the SPI NOR, which
holds both the pictures and the code that runs from there. The pictures cost
nothing in internal flash.

## How the pieces fit together

1. **Clock** – `ClockConfig::hrc_150mhz()`; `HCLK_HZ` in `main.rs` must match.
2. **SDRAM** – 8 MB at `0x8000_0000`. At 150 MHz the controller must run at
   `HCLK/2` (`ClkDiv::Div2`); `Div1` is only specified up to 140 MHz and reads
   back garbage beyond that. The chip's *self-test* runs first: one write /
   read-back word every 4 KB of the region this firmware claims. Keep it – a
   mis-clocked chip does not fault where it is touched, it faults much later,
   inside an allocator that trusts what it reads.
3. **Heap** – `embedded-alloc` over the 4 MB behind the framebuffers. It must be
   initialised *after* the SDRAM: `Heap::init` writes its free-list header into
   the window immediately, and doing that before the chip is clocked leaves that
   header as garbage.
4. **LCDC** – layer 0 covers the panel, `auto_restart` keeps refreshing. The two
   framebuffers swap at the end of every frame: the loop waits for `FRAMEDONE`,
   then repoints layer 0 in the blanking interval, so the panel never tears.
5. **Slint** – no std, no GPU. `MinimalSoftwareWindow` with
   `RepaintBufferType::SwappedBuffers` renders the whole scene into the back
   buffer (an `&mut [Rgb565Pixel]` straight over SDRAM) whenever something
   changed.

Each iteration calls `update_timers_and_animations()` (animations advance using
`time::now_ms()`), hands any input event to the window, and draws only if needed.

## Why embassy, and where the time comes from

The HAL's `embassy` feature is on: `hal::init` installs an `embassy_time` driver
on the SysTick at 1 kHz, and that one tick feeds everything -- Slint animations
(`time::now_ms()` is now `embassy_time::Instant`), task timers, and the idle
WFI. Nothing else may touch the SysTick, which is why `src/time.rs` reads the
driver instead of owning the exception.

The reason for an executor at all is that a picture does not load instantly:
300 KB out of a SPI flash plus 150k pixels of expansion is hundreds of
milliseconds, and doing that inside the UI loop is exactly what makes a UI feel
frozen. So:

- `main` (an `#[embassy_executor::main]` future) keeps the Slint loop: timers,
  input, drawing, and picking up finished pictures. Slint is built
  `unsafe-single-threaded`, so every Slint call stays on this one future -- which
  is also why it is *not* spawned as a task (tasks must be `Send`, and Slint
  handles are not).
- The `loader` task owns the slow part. It receives a picture index on the
  `REQUESTS` channel, reads and expands it **in chunks, yielding between them**
  (32 KB of flash, then 16 K pixels), and pushes the result onto `PICTURES`.
  The UI drains that channel once per frame, so a load costs the UI nothing but
  a late picture.
- The channels are `static`s of capacity 4 and 1: a request nobody has served yet
  is worthless, and only the newest finished picture matters, so both ends use
  `try_send` / `try_receive` and never block.
- `Picture` carries a plain `Vec<Rgb8Pixel>` rather than a Slint
  `SharedPixelBuffer`, because the latter is not `Send` and cannot cross the
  channel; the UI copies it into a buffer Slint owns when it arrives.

## One ELF, two memories: XiP

The 16 MB W25Q128 on the Flash-SPI pads is mapped by the SFC at `0x7000_0000`,
and the chip can take instructions and data straight out of that window. The
demo uses it for both, and everything stays in the one ELF:

- `memory.x` declares a third region, `SFC_XIP` at `0x7020_0000` (flash offset
  2 MiB), and puts every `.sfc_xip*` input section there.
- `build.rs` rasterises every `assets/*.svg` with `resvg` (2x, then downscaled)
  into `assets/build/raster/`, and `tools/make_gallery.py` turns those plus any
  `assets/*.png|jpg` into raw big-endian RGB565 at 480x320: scaled to fit and
  centred on a `--background` colour, so nothing is cropped or stretched at
  runtime. The bytes go behind a small table, and `build.rs` emits a `GALLERY`
  static with `#[link_section = ".sfc_xip.gallery"]`. They are in the ELF, at an
  address in the SPI NOR, and cost the internal flash nothing. No artwork? Three
  synthetic pictures are generated on top (`--synthetic`), so the gallery is
  never empty.
- `src/xip.rs` puts *code* there as well: a self-contained RGB565 -> RGB888
  expander, reached through the function-pointer table at the start of the
  image.
- At runtime `src/assets.rs` reads the table and the pixels through the window
  (the AHB window only serves word accesses, hence `Sfc::read`), expands them
  into a `SharedPixelBuffer` in the SDRAM heap and hands the result to the UI as
  a `slint::Image`. Only the picture on screen is kept; the < / > buttons and a
  5 s slideshow step through the table.
- `cargo run` programs that section into the real flash with probe-rs'
  `swm341_spiflash` algorithm before flashing the firmware (see `tools/run.sh`).

Three rules for anything put there:

1. **No `bl` out.** `bl` reaches +/-16 MB and the internal flash is 1.8 GB away,
   so XIP functions are only ever called through the pointer in `XIP_TABLE`
   (loaded with `movw`/`movt`). Keep them self-contained: no panics, no slicing,
   no helpers from other crates -- otherwise the compiler plants a branch that
   cannot be encoded.
2. **Nothing there runs before `Sfc::new`.** Reading the window while the
   controller is unconfigured faults, so `xip::available()` is only called after
   the SFC is up, and it verifies the image (magic + one trial run) before
   anything trusts it. Blank or stale flash is reported, not executed -- the
   firmware falls back to its own copy of the expander.
3. **Word accesses only** for data; instruction fetch is fine.

XiP is not free: the expander's instructions are fetched over SPI, so the log
prints how long each picture took and where the code came from. If a picture
appears too slowly, set `USE_XIP_EXPANDER = false` in `src/assets.rs` (the
internal-flash copy is a plain loop) -- XiP earns its keep for bulk data and
cold code, not for a loop over 150k pixels. The load runs in a task either way,
so this changes how long the picture takes, not whether the UI stutters.

## Changing things

- **Pictures** – drop PNG/JPG/SVG files into `assets/` and rebuild; `cargo run`
  re-programs the SPI NOR automatically. Any size/aspect is accepted (scaled to
  fit `--size`, default 480x320, settable through the `GALLERY_SIZE` env var;
  `GALLERY_SYNTHETIC=0` drops the generated ones). Keep the pixel count
  moderate: one 480x320 picture costs ~300 KB in the NOR and ~460 KB of SDRAM
  heap while shown (RGB565 copy + RGB888 buffer), and the whole `.sfc_xip`
  section has to fit in the 4 MiB `SFC_XIP` region of `memory.x`.
- **Panel geometry** – `DISPLAY_WIDTH` / `DISPLAY_HEIGHT` in `main.rs` and the
  same numbers in `ui/main.slint`; the timing in `lcd.configure` follows the
  datasheet of the panel.
- **UI** – edit `ui/main.slint`, then rebuild. Labels are ASCII on purpose: a
  CJK font costs megabytes of flash. To add Chinese text, embed a *subsetted*
  font (only the glyphs actually used) and set `default-font-family` on the
  window.
- **Touch input** – implement `input::poll()`; see the example in that file. Send
  `PointerPressed` / `PointerMoved` / `PointerReleased` as logical coordinates.
- **Power** – the loop already parks the core with `wfi()` when nothing is
  animating; the 1 ms SysTick wakes it again.

## Notes and gotchas

- One frame is 800x480x2 = 750 KB; Slint repaints all of it every time something
  changes. Expect 10-20 fps for a busy scene at 150 MHz. Switch to
  `RepaintBufferType::ReusedBuffer` plus dirty-region bookkeeping (or the SWM341
  DMA2D) if you need more.
- Slint is built with `unsafe-single-threaded`: every Slint call has to come
  from one thread, never from an interrupt handler (input adapters must hand the
  event to the loop, which dispatches it).
- `rprintln!` from inside a hard fault handler does not work: when something
  crashes before "UI up" appears, read the backtrace's faulting address and look
  it up with `arm-none-eabi-objdump -d` – that is how the two SDRAM bugs above
  were found.
