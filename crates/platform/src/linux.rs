//! Linux backend — US-1.2 spike target.
//! X11:     XShm / XComposite grabs, XGrabKey shortcuts.
//! Wayland: xdg-desktop-portal Screenshot + ScreenCast (PipeWire stream),
//!          GlobalShortcuts portal. Bar placement: wlr-layer-shell where available.
//! Encode:  VA-API when present (dlopen), software fallback otherwise.

use crate::stub::StubScreenSource;
use cb_core::*;

pub fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

pub fn backend_name() -> &'static str {
    if is_wayland() {
        "Linux · Wayland portal (todo)"
    } else {
        "Linux · X11 (todo)"
    }
}

pub struct LinuxScreenSource {
    inner: StubScreenSource,
}

impl LinuxScreenSource {
    pub fn new() -> Self {
        Self {
            inner: StubScreenSource,
        }
    }
}

impl ScreenSource for LinuxScreenSource {
    fn displays(&self) -> CaptureResult<Vec<DisplayInfo>> {
        self.inner.displays()
    }
    fn windows(&self) -> CaptureResult<Vec<WindowInfo>> {
        self.inner.windows()
    }
    fn grab(&self, target: &CaptureTarget) -> CaptureResult<Frame> {
        // TODO(US-1.2): X11 XShmGetImage, or the Screenshot portal on Wayland.
        self.inner.grab(target)
    }
    fn start_stream(
        &self,
        target: &CaptureTarget,
        fps: u32,
        sink: Box<dyn FrameSink>,
    ) -> CaptureResult<Box<dyn StreamHandle>> {
        self.inner.start_stream(target, fps, sink)
    }
}
