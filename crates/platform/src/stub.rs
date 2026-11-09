//! A backend that does nothing. Used as the starting point for each OS module
//! and on unsupported targets.

#![allow(dead_code)]

use cb_core::*;

pub struct StubScreenSource;

impl ScreenSource for StubScreenSource {
    fn displays(&self) -> CaptureResult<Vec<DisplayInfo>> {
        Err(CaptureError::NotImplemented("displays"))
    }
    fn windows(&self) -> CaptureResult<Vec<WindowInfo>> {
        Err(CaptureError::NotImplemented("windows"))
    }
    fn grab(&self, _target: &CaptureTarget) -> CaptureResult<Frame> {
        Err(CaptureError::NotImplemented("grab"))
    }
    fn start_recording(
        &self,
        _target: &CaptureTarget,
        _fps: u32,
        _audio: AudioSources,
        _path: &std::path::Path,
        _done: Box<dyn FnOnce(CaptureResult<Recording>) + Send>,
    ) -> CaptureResult<Box<dyn StreamHandle>> {
        Err(CaptureError::NotImplemented("recording"))
    }
}
