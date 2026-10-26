//! Monotonic millisecond clock for Slint.
//!
//! Slint needs `Platform::duration_since_start` to animate properties; on bare
//! metal there is no clock to ask, so this module reads the one the HAL sets up:
//! with its `embassy` feature on, `hal::init` installs an `embassy_time` driver
//! on the SysTick at 1 kHz, and everything time-based -- Slint animations, the
//! task timers, this clock -- comes from that single source.
//!
//! (Before embassy this module owned the SysTick itself. It cannot any more: one
//! exception, one owner, and the executor needs the tick as well.)

use core::time::Duration;

/// Milliseconds since the HAL came up; wraps after roughly 49 days.
pub fn now_ms() -> u32 {
    embassy_time::Instant::now().as_millis() as u32
}

/// Same thing as a [`Duration`], as wanted by
/// [`slint::platform::Platform::duration_since_start`].
pub fn since_start() -> Duration {
    Duration::from_millis(now_ms() as u64)
}
