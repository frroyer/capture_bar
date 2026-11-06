//! Windows backend — US-1.2 spike target.
//! Capture: Windows.Graphics.Capture (DXGI Desktop Duplication fallback).
//! Encode:  Media Foundation H.264 (hardware MFT).
//! Exclude the bar from captures: SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE).

use crate::stub::StubScreenSource;
use cb_core::*;

pub struct WinScreenSource {
    inner: StubScreenSource,
}

impl WinScreenSource {
    pub fn new() -> Self {
        Self {
            inner: StubScreenSource,
        }
    }
}

impl ScreenSource for WinScreenSource {
    fn displays(&self) -> CaptureResult<Vec<DisplayInfo>> {
        self.inner.displays()
    }
    fn windows(&self) -> CaptureResult<Vec<WindowInfo>> {
        self.inner.windows()
    }
    fn grab(&self, target: &CaptureTarget) -> CaptureResult<Frame> {
        // TODO(US-1.2): GraphicsCaptureItem for the target -> one frame -> RGBA.
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
