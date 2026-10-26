//! Slint on the Synwit SWM341.
//!
//! Bring-up flow, all of it before Slint sees anything:
//!   1. core clock to 150 MHz, SysTick running at 1 kHz,
//!   2. SDRAM up (framebuffers and the allocator live there),
//!   3. LCDC pads muxed, panel reset released, backlight on,
//!   4. LCDC fed 800x480 RGB565 timing and pointed at framebuffer #0,
//!   5. SFC up: the demo pictures are part of this ELF but live at a SPI NOR
//!      address, and are read through the SFC's AHB window (XiP),
//!   6. Slint given a `Platform` and a super loop that paints into whichever
//!      framebuffer is not currently being scanned out.

#![no_std]
#![no_main]

extern crate alloc;

mod assets;
mod input;
mod platform;
mod time;
mod xip;

use alloc::boxed::Box;
use core::slice::from_raw_parts_mut;

use cortex_m::asm::{delay, dsb};
use embassy_executor::Spawner;
use embassy_futures::yield_now;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver};
use panic_halt as _;
use rtt_target::{rprintln, rtt_init_print};

use slint::platform::software_renderer::Rgb565Pixel;
use slint::{Image, Rgb8Pixel, SharedPixelBuffer};

use swm341_hal as hal;
use hal::gpio::{Level, Output, OutputConfig};
use hal::lcdc::{Config, HvTiming, Lcd, LcdPins, LayerConfig, PixelFormat};
use hal::sdramc::{CasLatency, ClkDiv, SdramPins, Sdramc};

slint::include_modules!();

/// Core clock. Must match `ClockConfig::hrc_150mhz()` below.
const HCLK_HZ: u32 = 150_000_000;

/// Panel geometry.
const DISPLAY_WIDTH: usize = 800;
const DISPLAY_HEIGHT: usize = 480;

/// Two RGB565 framebuffers, 750 KB each, both inside SDRAM.
const FRAMEBUFFER_BYTES: usize = DISPLAY_WIDTH * DISPLAY_HEIGHT * 2;
const FRAMEBUFFERS: [u32; 2] = [
    hal::sdramc::SDRAM_BASE as u32,
    hal::sdramc::SDRAM_BASE as u32 + FRAMEBUFFER_BYTES as u32,
];

/// Slint's allocator takes the 4 MB right behind the framebuffers.
const HEAP_START: usize = hal::sdramc::SDRAM_BASE + 2 * FRAMEBUFFER_BYTES;
const HEAP_SIZE: usize = 4 * 1024 * 1024;

#[global_allocator]
static HEAP: embedded_alloc::Heap = embedded_alloc::Heap::empty();

/// UI -> loader: which picture is wanted next.
///
/// Small on purpose: a request that arrives while an older one is still pending
/// is worthless, so the UI uses `try_send` and lets overflows go.
static REQUESTS: Channel<CriticalSectionRawMutex, usize, 4> = Channel::new();
/// loader -> UI: a finished picture. Only the newest one is ever interesting,
/// hence capacity 1.
static PICTURES: Channel<CriticalSectionRawMutex, assets::Picture, 1> = Channel::new();

/// The loader task: reads and expands pictures out of the SPI NOR, which takes
/// long enough to be visible on screen if it ran inside the UI loop.
#[embassy_executor::task]
async fn loader(requests: Receiver<'static, CriticalSectionRawMutex, usize, 4>) {
    loop {
        let index = requests.receive().await;
        match assets::load(index).await {
            Some(picture) => {
                // Nobody waits for a stale picture, so never block on send.
                let _ = PICTURES.sender().try_send(picture);
            }
            None => rprintln!("gallery: picture {} could not be loaded", index),
        }
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    rtt_init_print!();
    rprintln!("swm341-slint: booting");

    // `hal::init` installs the embassy time driver on the SysTick, which is
    // where `time::now_ms`, the Slint animations and every task timer come from.
    let p = hal::init(hal::Config {
        clock: hal::clock::ClockConfig::hrc_150mhz(),
    });
    rprintln!("HAL up at {} MHz", HCLK_HZ / 1_000_000);

    // Grab the peripheral handles first: moving `p.Pxy` fields out below would
    // otherwise forbid borrowing `p` again.
    let lcd_peri = p.lcd();
    let sdram_peri = p.sdramc();
    let sfc_peri = p.sfc();

    // ---------------------------------------------------------------- SDRAM --
    let sdram = Sdramc::new_with_pins(
        sdram_peri,
        SdramPins::default(),
        
        // At 150 MHz the chip has to run at HCLK/2: the controller only accepts
        // HCLK/1 up to 140 MHz, and out-of-spec clocks read back garbage
        // (typically in the middle of a heap allocation).
        hal::sdramc::Config::default()
            .clk_div(ClkDiv::Div2)
            .cas_latency(CasLatency::Latency3)
            .trc(8),
    );
    rprintln!(
        "SDRAM up: {} bytes @ 0x{:08X}",
        sdram.size_bytes(),
        sdram.base_address()
    );

    // The heap is about to be handed to an allocator that trusts every word it
    // reads back, so check the bus first: a chip running out of spec does not
    // fail loudly, it returns garbage and the next indirect call lands at some
    // random address inside SDRAM (HardFault with a nonsense return address).
    if !sdram_probe(HEAP_START + HEAP_SIZE) {
        rprintln!("SDRAM self-test FAILED -- halting");
        loop {}
    }
    rprintln!("SDRAM self-test OK");

    // Hand the memory to the allocator only *now*: `Heap::init` writes its first
    // free-list header into the window right away, so calling it before the chip
    // is clocked would leave that header as garbage -- and the first allocation
    // would then index the free list with whatever noise it reads back.
    unsafe { HEAP.init(HEAP_START, HEAP_SIZE) };

    // ------------------------------------------------------------------ LCD --
    let lcd = Lcd::new_with_pins(
        lcd_peri,
        LcdPins::default(),
    );

    let mut lcd_rst = Output::new(p.PD1, OutputConfig::default().initial_level(Level::Low));
    delay(HCLK_HZ / 1000);
    lcd_rst.set_high();

    // Backlight: PD9 on the 100-pin board, PB13 on the 64-pin one.
    let mut lcd_bl = Output::new(p.PD9, OutputConfig::default().initial_level(Level::High));
    let _ = &mut lcd_bl;

    // DOTCLK = HCLK / (clock_div + 2) = 30 MHz.
    lcd.configure(&Config {
        clock_div: 3,
        format: PixelFormat::Rgb565,
        invert_clock: true,
        invert_vsync: false,
        invert_hsync: false,
        serial: false,
        mpu_enable: false,
        // Keep the panel refreshed without having to restart every frame.
        auto_restart: true,
        h: HvTiming {
            sync: 5,
            back_porch: 46,
            pixels: DISPLAY_WIDTH as u16,
            front_porch: 64,
        },
        v: HvTiming {
            sync: 5,
            back_porch: 23,
            pixels: DISPLAY_HEIGHT as u16,
            front_porch: 22,
        },
        bg_color: 0x0000,
    });

    // Black out both buffers before anything is scanned out: SDRAM keeps stale
    // contents across an SWD reflash.
    for address in FRAMEBUFFERS {
        let words = unsafe {
            from_raw_parts_mut(address as *mut u16, DISPLAY_WIDTH * DISPLAY_HEIGHT)
        };
        words.fill(0);
    }

    lcd.clear_frame_done();
    lcd.set_layer(0, &layer_config(FRAMEBUFFERS[0]));
    lcd.start_frame();
    let mut front: usize = 0;
    rprintln!("LCD scanning out {}x{} RGB565", DISPLAY_WIDTH, DISPLAY_HEIGHT);

    // ---------------------------------------------------- external NOR (SFC) --
    // The pictures of the gallery demo are linked into the `.sfc_xip` section,
    // i.e. into this very ELF at a SPI NOR address, and are read through the
    // SFC's AHB window: no copy into internal flash, no decoding. `cargo run`
    // programs that section into the flash before starting this.
    // Quad read: the same window also feeds the instruction fetch of whatever
    // runs from there, so the extra three data lines matter twice.
    match assets::init(hal::sfc::Sfc::new_with_config(
        sfc_peri,
        hal::sfc::Config::quad(),
    )) {
        Ok(count) => rprintln!("assets: {} pictures in external flash", count),
        Err(message) => rprintln!("assets: {}", message),
    }

    // Only now is the flash window mapped, so only now may the XIP image be
    // looked at: reading `0x7020_0000` before `Sfc::new` would fault.
    rprintln!(
        "XIP: code in SPI flash {}",
        if xip::available() {
            "ok (running from there)"
        } else {
            "unavailable -- using the internal-flash copy"
        }
    );

    // --------------------------------------------------------------- Slint --
    let window = platform::SwmWindow::new(platform::REPAINT_BUFFER_TYPE);
    window.set_size(slint::PhysicalSize::new(
        DISPLAY_WIDTH as u32,
        DISPLAY_HEIGHT as u32,
    ));

    slint::platform::set_platform(Box::new(platform::SwmPlatform::new(window.clone())))
        .expect("Slint platform installed exactly once");

    let ui = MainWindow::new().expect("the UI fits the panel");
    ui.on_tick(|| {
        rprintln!("uptime: {} ms", time::now_ms());
    });

    // The gallery: the UI only ever *asks* for a picture (over `REQUESTS`) and
    // picks up whatever the loader task has finished (from `PICTURES`), so a
    // load never blocks a frame.
    spawner
        .spawn(loader(REQUESTS.receiver()))
        .expect("loader task spawned once");

    ui.set_picture_count(assets::count() as i32);
    ui.on_next_picture(|| {
        if let Some(index) = assets::step_index(1) {
            let _ = REQUESTS.sender().try_send(index);
        }
    });
    ui.on_prev_picture(|| {
        if let Some(index) = assets::step_index(-1) {
            let _ = REQUESTS.sender().try_send(index);
        }
    });

    // Ask for the opening picture; it arrives a few frames later.
    let _ = REQUESTS.sender().try_send(0);

    ui.show().expect("the window adapter is already set");
    rprintln!("UI up -- entering the event loop");

    loop {
        // Let Slint advance animations and run magic timers.
        slint::platform::update_timers_and_animations();

        if let Some(event) = input::poll() {
            window.dispatch_event(event);
        }

        // A picture that finished loading: copy it into a buffer Slint owns and
        // put it on screen. All Slint calls stay on this task, which is what the
        // `unsafe-single-threaded` build requires.
        if let Ok(picture) = PICTURES.receiver().try_receive() {
            let mut buffer =
                SharedPixelBuffer::<Rgb8Pixel>::new(picture.width, picture.height);
            buffer.make_mut_slice().copy_from_slice(&picture.pixels);
            ui.set_picture(Image::from_rgb8(buffer));
            ui.set_picture_index(picture.index as i32);
            rprintln!(
                "gallery: showing picture {} (expanded {})",
                picture.index + 1,
                if picture.via_xip {
                    "from SPI flash"
                } else {
                    "from internal flash"
                }
            );
        }

        window.draw_if_needed(|renderer| {
            let back = front ^ 1;
            let buffer = unsafe {
                from_raw_parts_mut(
                    FRAMEBUFFERS[back] as *mut Rgb565Pixel,
                    DISPLAY_WIDTH * DISPLAY_HEIGHT,
                )
            };

            renderer.render(buffer, DISPLAY_WIDTH);

            // Everything the CPU wrote has to be visible to the LCDC's DMA
            // before the next frame picks it up.
            dsb();

            // Wait for the outgoing frame to finish so the address switch lands
            // in the blanking interval -- otherwise the panel tears.
            while !lcd.is_frame_done() {}
            lcd.clear_frame_done();
            lcd.set_layer(0, &layer_config(FRAMEBUFFERS[back]));
            front = back;
        });

        // Hand the core back to the executor: this is what lets the loader task
        // run, and it parks the core (WFI) when no task has work -- the 1 ms
        // tick of the time driver wakes it again.
        yield_now().await;
    }
}

/// Write / read-back check over every region this firmware claims in SDRAM
/// (both framebuffers and the heap).
///
/// One word every 4 KB is enough to catch a mis-clocked chip while keeping the
/// test fast; `end` is the first address past the last byte we use.
fn sdram_probe(end: usize) -> bool {
    let start = hal::sdramc::SDRAM_BASE;
    let mut address = start;

    while address < end {
        let pattern = 0x5A5A_A5A5u32 ^ address as u32;
        let slot = address as *mut u32;
        unsafe {
            core::ptr::write_volatile(slot, pattern);
        }
        let read_back = unsafe { core::ptr::read_volatile(slot) };
        if read_back != pattern {
            rprintln!(
                "SDRAM mismatch @ 0x{:08X}: wrote 0x{:08X}, read 0x{:08X}",
                address,
                pattern,
                read_back
            );
            return false;
        }

        address += 4096;
        if (address - start) % (512 * 1024) == 0 {
            rprintln!("  probed 0x{:08X}", address);
        }
    }
    true
}

/// Point layer 0 at `framebuffer` and stretch it over the whole panel.
fn layer_config(framebuffer: u32) -> LayerConfig {
    LayerConfig {
        enable: true,
        x_start: 0,
        x_stop: DISPLAY_WIDTH as u16 - 1,
        y_start: 0,
        y_stop: DISPLAY_HEIGHT as u16 - 1,
        framebuffer,
        stride_bytes: DISPLAY_WIDTH as u32 * 2,
        alpha: 255,
        color_key: None,
    }
}
