//! Input source for the Slint event loop.
//!
//! The demo board this template targets has a plain RGB panel with no touch
//! panel wired up, so [`poll`] returns `None` and the UI is driven by its own
//! `Timer`. Plug your controller in here: read the device and return the events
//! that happened since the previous call.
//!
//! A capacitive controller such as the GT911 (I2C, INT pad) turns into:

//! ```ignore
//! pub fn poll() -> Option<slint::platform::WindowEvent> {
//!     let report = crate::TOUCH.read()?;              // your driver
//!     Some(slint::platform::WindowEvent::PointerPressed {
//!         position: slint::LogicalPosition::new(report.x as f32, report.y as f32),
//!         button: slint::platform::PointerEventButton::Left,
//!     })
//! }
//! ```
//!
//! ...and the release is reported with `PointerReleased`. If your controller
//! streams positions while the finger is down, also send
//! `WindowEvent::PointerMoved` for each one.

/// Fetch the next pending input event, if any.
pub fn poll() -> Option<slint::platform::WindowEvent> {
    None
}
