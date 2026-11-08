//! Encoders: PNG (FR-13), GIF and PDF (US-6.7). MP4 (US-5.1) and the background job
//! queue (US-6.2) come next.

mod anim;
mod pdf;

pub use anim::write_gif;
pub use pdf::write_pdf;

use cb_core::Frame;
use std::io::{BufRead, Seek, Write};

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("frame buffer size does not match {0}x{1} RGBA")]
    BadFrame(u32, u32),
    #[error(transparent)]
    Png(#[from] png::EncodingError),
    #[error(transparent)]
    Decode(#[from] png::DecodingError),
    #[error("not an 8-bit RGB or RGBA image")]
    Unsupported,
    #[error(transparent)]
    Gif(#[from] gif::EncodingError),
    #[error("nothing to export")]
    Nothing,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

fn check_size(frame: &Frame) -> Result<(), EncodeError> {
    let expected = frame.width as usize * frame.height as usize * 4;
    if frame.rgba.len() != expected {
        return Err(EncodeError::BadFrame(frame.width, frame.height));
    }
    Ok(())
}

/// The source pixels that destination pixel `i` covers, along one axis.
fn covered(i: usize, dst: usize, src: usize) -> std::ops::Range<usize> {
    let start = i * src / dst;
    start..((i + 1) * src / dst).max(start + 1)
}

/// A copy of `frame` that fits in `max_width` x `max_height`, keeping its shape. Each pixel
/// averages the ones it covers, so small text stays recognisable. Never enlarges.
pub fn thumbnail(frame: &Frame, max_width: u32, max_height: u32) -> Result<Frame, EncodeError> {
    check_size(frame)?;
    if frame.rgba.is_empty() {
        return Err(EncodeError::BadFrame(frame.width, frame.height));
    }
    let scale = f64::min(
        f64::from(max_width) / f64::from(frame.width),
        f64::from(max_height) / f64::from(frame.height),
    )
    .min(1.0);
    let fit = |side: u32| ((f64::from(side) * scale).round() as u32).max(1);
    let (width, height) = (fit(frame.width), fit(frame.height));

    let (src_w, src_h) = (frame.width as usize, frame.height as usize);
    let (dst_w, dst_h) = (width as usize, height as usize);
    let mut rgba = Vec::with_capacity(dst_w * dst_h * 4);
    for y in 0..dst_h {
        let rows = covered(y, dst_h, src_h);
        for x in 0..dst_w {
            let columns = covered(x, dst_w, src_w);
            let mut sum = [0u64; 4];
            for row in rows.clone() {
                let line = &frame.rgba[(row * src_w + columns.start) * 4..][..columns.len() * 4];
                for (channel, value) in line.iter().enumerate() {
                    sum[channel % 4] += u64::from(*value);
                }
            }
            let count = (rows.len() * columns.len()) as u64;
            rgba.extend(sum.map(|total| (total / count) as u8));
        }
    }
    Ok(Frame {
        width,
        height,
        rgba,
        captured_at: frame.captured_at,
    })
}

/// Write a frame as lossless PNG.
pub fn write_png<W: Write>(frame: &Frame, out: W) -> Result<(), EncodeError> {
    check_size(frame)?;
    let mut enc = png::Encoder::new(out, frame.width, frame.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    let mut writer = enc.write_header()?;
    writer.write_image_data(&frame.rgba)?;
    writer.finish()?;
    Ok(())
}

/// Read back a PNG this app wrote (or any 8-bit RGB/RGBA one), e.g. to copy it (FR-23).
/// The capture time is not stored in the file: it becomes now.
pub fn read_png<R: BufRead + Seek>(input: R) -> Result<Frame, EncodeError> {
    let mut decoder = png::Decoder::new(input);
    decoder.set_transformations(
        png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
    );
    let mut reader = decoder.read_info()?;
    if reader.output_color_type() != (png::ColorType::Rgba, png::BitDepth::Eight) {
        return Err(EncodeError::Unsupported);
    }
    let mut rgba = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or(EncodeError::Unsupported)?
    ];
    let info = reader.next_frame(&mut rgba)?;
    rgba.truncate(info.buffer_size());
    Ok(Frame {
        width: info.width,
        height: info.height,
        rgba,
        captured_at: std::time::SystemTime::now(),
    })
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
    fn a_png_reads_back_as_the_same_pixels() {
        let rgba: Vec<u8> = (0..24).collect();
        let mut out = Vec::new();
        write_png(&frame(3, 2, rgba.clone()), &mut out).unwrap();
        let back = read_png(std::io::Cursor::new(out)).unwrap();
        assert_eq!((back.width, back.height, back.rgba), (3, 2, rgba));
        assert!(read_png(std::io::Cursor::new(b"not a png".to_vec())).is_err());
    }

    fn frame(width: u32, height: u32, rgba: Vec<u8>) -> Frame {
        Frame {
            width,
            height,
            rgba,
            captured_at: SystemTime::now(),
        }
    }

    #[test]
    fn thumbnail_averages_the_pixels_it_covers() {
        let rgba = [[0, 0, 0, 255], [200, 100, 50, 255], [9, 9, 9, 255], [9; 4]].concat();
        let thumb = thumbnail(&frame(4, 1, rgba), 2, 2).unwrap();
        assert_eq!((thumb.width, thumb.height), (2, 1));
        assert_eq!(thumb.rgba, [100, 50, 25, 255, 9, 9, 9, 132]);
    }

    #[test]
    fn thumbnail_keeps_the_shape_and_never_enlarges() {
        let size = |w: u32, h: u32, max: (u32, u32)| {
            let thumb = thumbnail(&frame(w, h, vec![7; (w * h * 4) as usize]), max.0, max.1);
            let thumb = thumb.unwrap();
            assert_eq!(thumb.rgba.len(), (thumb.width * thumb.height * 4) as usize);
            (thumb.width, thumb.height)
        };
        assert_eq!(size(3840, 2160, (320, 200)), (320, 180));
        assert_eq!(size(1000, 2000, (320, 200)), (100, 200));
        assert_eq!(size(10, 5, (320, 200)), (10, 5));
        assert_eq!(
            size(4000, 1, (320, 200)),
            (320, 1),
            "a sliver stays visible"
        );
    }

    #[test]
    fn thumbnail_rejects_frames_it_cannot_read() {
        assert!(thumbnail(&frame(4, 4, vec![0; 3]), 2, 2).is_err());
        assert!(thumbnail(&frame(0, 0, Vec::new()), 2, 2).is_err());
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
