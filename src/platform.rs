//! The [`slint::platform::Platform`] implementation for SWM341.
//!
//! All the operating-system duties Slint normally relies on end up in this one
//! struct: handing out a window adapter and telling the time. Everything else
//! stays outside of Slint, in the super loop of `main`.

use alloc::rc::Rc;
use core::time::Duration;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};

/// The single window this firmware can show.
///
/// `SwappedBuffers` tells Slint it must repaint the whole frame every time,
/// because the pixel contents of the buffer it draws into are lost once it has
/// been displayed (rendering goes into whichever buffer is not being scanned
/// out). The alternative, `ReusedBuffer`, only repaints dirty regions and needs
/// the front buffer to be kept in sync.
pub type SwmWindow = MinimalSoftwareWindow;

/// The repaint strategy of every window handed out by this template.
pub const REPAINT_BUFFER_TYPE: RepaintBufferType = RepaintBufferType::SwappedBuffers;

pub struct SwmPlatform {
    window: Rc<SwmWindow>,
}

impl SwmPlatform {
    pub fn new(window: Rc<SwmWindow>) -> Self {
        Self { window }
    }
}

impl Platform for SwmPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        // There is exactly one panel on this board, so every component shares
        // the same adapter.
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        crate::time::since_start()
    }
}
