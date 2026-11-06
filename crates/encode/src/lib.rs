//! Encoders. PNG works today (FR-13). MP4 (US-5.1), GIF and PDF (US-6.7)
//! and the background job queue (US-6.2) come next.

use cb_core::Frame;
use std::io::Write;

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("frame buffer size does not match {0}x{1} RGBA")]
    BadFrame(u32, u32),
    #[error(transparent)]
    Png(#[from] png::EncodingError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Write a frame as lossless PNG.
pub fn write_png<W: Write>(frame: &Frame, out: W) -> Result<(), EncodeError> {
    let expected = frame.width as usize * frame.height as usize * 4;
    if frame.rgba.len() != expected {
        return Err(EncodeError::BadFrame(frame.width, frame.height));
    }
    let mut enc = png::Encoder::new(out, frame.width, frame.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    let mut writer = enc.write_header()?;
    writer.write_image_data(&frame.rgba)?;
    writer.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    #[test]
    fn encodes_a_tiny_png() {
        let frame = Frame {
            width: 2,
            height: 2,
            rgba: vec![255; 16],
            captured_at: SystemTime::now(),
        };
        let mut out = Vec::new();
        write_png(&frame, &mut out).unwrap();
        assert_eq!(&out[1..4], b"PNG");
    }

    #[test]
    fn rejects_bad_size() {
        let frame = Frame {
            width: 4,
            height: 4,
            rgba: vec![0; 3],
            captured_at: SystemTime::now(),
        };
        assert!(write_png(&frame, Vec::new()).is_err());
    }
}
