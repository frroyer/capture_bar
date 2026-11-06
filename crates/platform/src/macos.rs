//! macOS backend — US-1.2 spike target.
//! Capture: ScreenCaptureKit (macOS 12.3+), SCContentFilter excluding the bar.
//! Encode:  VideoToolbox H.264 / HEVC via AVAssetWriter.
//! Needs Screen Recording permission -> app must ship as a signed .app (US-9.2).

use crate::stub::StubScreenSource;
use cb_core::*;

pub struct MacScreenSource {
    inner: StubScreenSource,
}

impl MacScreenSource {
    pub fn new() -> Self {
        Self {
            inner: StubScreenSource,
        }
    }
}

impl ScreenSource for MacScreenSource {
    fn displays(&self) -> CaptureResult<Vec<DisplayInfo>> {
        self.inner.displays()
    }
    fn windows(&self) -> CaptureResult<Vec<WindowInfo>> {
        self.inner.windows()
    }
    fn grab(&self, target: &CaptureTarget) -> CaptureResult<Frame> {
        // TODO(US-1.2): SCScreenshotManager::captureImage for the filter.
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
