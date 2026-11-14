//! Where the presenter's face is in the webcam track (US-10.6, DR-23), once a second, by
//! the face detector Windows ships (Windows.Media.FaceAnalysis): nothing to install.

use crate::windows::ensure_mta;
use cb_core::Rect;
use std::path::Path;
use windows::core::Result as WinResult;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::FaceAnalysis::FaceDetector;
use windows::Security::Cryptography::CryptographicBuffer;

/// The largest face of each second of the video at `path`, in its pixels: `(t_ms from
/// the video's start, face)`, `None` where nobody shows. Slow: call it from a worker.
pub(crate) fn face_boxes(path: &Path) -> WinResult<Vec<(u64, Option<Rect>)>> {
    ensure_mta();
    if !FaceDetector::IsSupported()? {
        return Err(windows::core::Error::from(
            windows::Win32::Foundation::E_NOTIMPL,
        ));
    }
    let detector = FaceDetector::CreateAsync()?.join()?;
    let mut faces = Vec::new();
    let mut failure = None;
    crate::windows_mp4::video_clip(path, 1, &mut |frame, _| {
        let t_ms = faces.len() as u64 * 1000;
        match largest_face(&detector, &frame) {
            Ok(face) => {
                faces.push((t_ms, face));
                true
            }
            Err(e) => {
                failure = Some(e);
                false
            }
        }
    })?;
    match failure {
        Some(e) => Err(e),
        None => Ok(faces),
    }
}

fn largest_face(detector: &FaceDetector, frame: &cb_core::Frame) -> WinResult<Option<Rect>> {
    // The detector reads grey or NV12 pictures.
    let grey: Vec<u8> = frame
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|[r, g, b, _]| {
            ((u32::from(*r) * 77 + u32::from(*g) * 150 + u32::from(*b) * 29) >> 8) as u8
        })
        .collect();
    let buffer = CryptographicBuffer::CreateFromByteArray(&grey)?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &buffer,
        BitmapPixelFormat::Gray8,
        frame.width as i32,
        frame.height as i32,
    )?;
    let found = detector.DetectFacesAsync(&bitmap)?.join()?;
    let mut largest: Option<Rect> = None;
    for i in 0..found.Size()? {
        let b = found.GetAt(i)?.FaceBox()?;
        let face = Rect {
            x: b.X as i32,
            y: b.Y as i32,
            width: b.Width,
            height: b.Height,
        };
        if largest.is_none_or(|l| face.width * face.height > l.width * l.height) {
            largest = Some(face);
        }
    }
    Ok(largest)
}
