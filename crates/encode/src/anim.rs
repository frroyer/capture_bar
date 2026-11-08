//! Animated GIF from a clip's frames (FR-18, US-6.7). Screen recordings change little from
//! one frame to the next, so each frame after the first carries only the rectangle that
//! changed, and a frame that changed nothing just lengthens the one before. That, not the
//! palette, is what keeps a clip small.

use crate::{check_size, EncodeError};
use cb_core::Frame;
use std::io::Write;

/// NeuQuant sampling: 1 is best and slowest, 30 fastest. 10 is the gif crate's default.
const QUANTIZE_SPEED: i32 = 10;

/// The smallest rectangle `(x, y, width, height)` holding every pixel that differs.
fn changed_rect(before: &Frame, after: &Frame) -> Option<(u32, u32, u32, u32)> {
    let row = after.width as usize * 4;
    let mut rows = (0..after.height as usize)
        .filter(|&y| before.rgba[y * row..(y + 1) * row] != after.rgba[y * row..(y + 1) * row]);
    let top = rows.next()?;
    let bottom = rows.next_back().unwrap_or(top);
    let (mut left, mut right) = (after.width as usize, 0);
    for y in top..=bottom {
        let line = y * row..(y + 1) * row;
        let pixels = before.rgba[line.clone()]
            .as_chunks::<4>()
            .0
            .iter()
            .zip(after.rgba[line].as_chunks::<4>().0.iter());
        for (x, (a, b)) in pixels.enumerate() {
            if a != b {
                left = left.min(x);
                right = right.max(x);
            }
        }
    }
    let span = |from: usize, to: usize| (to - from + 1) as u32;
    Some((
        left as u32,
        top as u32,
        span(left, right),
        span(top, bottom),
    ))
}

/// The pixels of `frame` inside a rectangle, as RGBA rows.
fn crop(frame: &Frame, (x, y, width, height): (u32, u32, u32, u32)) -> Vec<u8> {
    let row = frame.width as usize * 4;
    (y as usize..(y + height) as usize)
        .flat_map(|line| {
            let start = line * row + x as usize * 4;
            frame.rgba[start..start + width as usize * 4]
                .iter()
                .copied()
        })
        .collect()
}

/// Write `frames`, captured at `fps`, as a GIF that loops forever. All frames must have the
/// size of the first, at most 65535 pixels a side. Returns the number of GIF frames written.
pub fn write_gif<W: Write>(
    frames: impl IntoIterator<Item = Frame>,
    fps: u32,
    out: W,
) -> Result<usize, EncodeError> {
    let fps = fps.max(1);
    let mut frames = frames.into_iter();
    let first = frames.next().ok_or(EncodeError::Nothing)?;
    check_size(&first)?;
    let side =
        |n: u32| u16::try_from(n).map_err(|_| EncodeError::BadFrame(first.width, first.height));
    let (width, height) = (side(first.width)?, side(first.height)?);
    let mut encoder = gif::Encoder::new(out, width, height, &[])?;
    encoder.set_repeat(gif::Repeat::Infinite)?;

    // GIF delays are in hundredths of a second: frame n starts at n * 100 / fps.
    let start_of = |n: u64| (n * 100 / u64::from(fps)) as u16;
    let mut pending =
        gif::Frame::from_rgba_speed(width, height, &mut first.rgba.clone(), QUANTIZE_SPEED);
    let mut pending_from = 0u64;
    let mut previous = first;
    let mut written = 0;
    let mut n = 0u64;
    for frame in frames {
        n += 1;
        if (frame.width, frame.height) != (previous.width, previous.height) {
            return Err(EncodeError::BadFrame(frame.width, frame.height));
        }
        check_size(&frame)?;
        let Some(rect) = changed_rect(&previous, &frame) else {
            continue;
        };
        pending.delay = start_of(n).saturating_sub(start_of(pending_from));
        pending.dispose = gif::DisposalMethod::Keep;
        encoder.write_frame(&pending)?;
        written += 1;

        let (x, y, w, h) = rect;
        pending = gif::Frame::from_rgba_speed(
            w as u16,
            h as u16,
            &mut crop(&frame, rect),
            QUANTIZE_SPEED,
        );
        (pending.left, pending.top) = (x as u16, y as u16);
        pending_from = n;
        previous = frame;
    }
    // The last frame lasts one frame time, plus any unchanged frames after it.
    pending.delay = start_of(n + 1)
        .saturating_sub(start_of(pending_from))
        .max(1);
    pending.dispose = gif::DisposalMethod::Keep;
    encoder.write_frame(&pending)?;
    Ok(written + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn solid(width: u32, height: u32, rgb: [u8; 3]) -> Frame {
        Frame {
            width,
            height,
            rgba: [rgb[0], rgb[1], rgb[2], 255].repeat((width * height) as usize),
            captured_at: SystemTime::now(),
        }
    }

    fn paint(frame: &mut Frame, x: u32, y: u32, rgb: [u8; 3]) {
        let at = ((y * frame.width + x) * 4) as usize;
        frame.rgba[at..at + 3].copy_from_slice(&rgb);
    }

    /// Each frame of a GIF: its rectangle and its delay.
    fn read_back(bytes: &[u8]) -> Vec<((u16, u16, u16, u16), u16)> {
        let mut decoder = gif::DecodeOptions::new().read_info(bytes).unwrap();
        let mut frames = Vec::new();
        while let Some(f) = decoder.read_next_frame().unwrap() {
            frames.push(((f.left, f.top, f.width, f.height), f.delay));
        }
        frames
    }

    #[test]
    fn only_what_changed_is_written_and_still_frames_lengthen_the_last() {
        let first = solid(8, 6, [10, 20, 30]);
        let mut moved = first.clone();
        paint(&mut moved, 2, 1, [250, 0, 0]);
        paint(&mut moved, 5, 3, [0, 250, 0]);
        let frames = vec![first.clone(), first.clone(), moved.clone(), moved];
        let mut out = Vec::new();
        assert_eq!(write_gif(frames, 10, &mut out).unwrap(), 2);
        assert_eq!(
            read_back(&out),
            vec![((0, 0, 8, 6), 20), ((2, 1, 4, 3), 20)],
            "the still second frame doubles the first; the change is 2,1 to 5,3"
        );
    }

    #[test]
    fn mismatched_or_missing_frames_are_errors() {
        assert!(matches!(
            write_gif([], 10, Vec::new()),
            Err(EncodeError::Nothing)
        ));
        let frames = [solid(4, 4, [0; 3]), solid(4, 5, [0; 3])];
        assert!(write_gif(frames, 10, Vec::new()).is_err());
    }

    /// US-6.7 AC: a 10 s clip at 720p is at most 15 MB. A desktop-like scene: a busy,
    /// fixed backdrop, a cursor moving every frame and a panel redrawn every second.
    /// Slow in debug builds: `cargo test -p cb-encode --release -- --ignored`.
    #[test]
    #[ignore = "slow outside release builds"]
    fn ten_seconds_at_720p_fit_in_15_mb() {
        let (width, height, fps) = (1280u32, 720u32, 15u32);
        let mut frame = solid(width, height, [240, 240, 240]);
        for y in 0..height {
            for x in 0..width {
                if (x / 7 + y / 11) % 5 == 0 {
                    paint(&mut frame, x, y, [(x % 256) as u8, (y % 256) as u8, 90]);
                }
            }
        }
        let frames = (0..10 * fps).map(|n| {
            let mut f = frame.clone();
            for dy in 0..16 {
                for dx in 0..12 {
                    paint(&mut f, 100 + n * 7 + dx, 200 + n * 2 + dy, [0, 0, 0]);
                }
            }
            if (n / fps) % 2 == 1 {
                for y in 400..600 {
                    for x in 600..1000 {
                        paint(
                            &mut f,
                            x,
                            y,
                            [((x * y) % 256) as u8, 40, ((x + y) % 256) as u8],
                        );
                    }
                }
            }
            f
        });
        let mut out = Vec::new();
        write_gif(frames, fps, &mut out).unwrap();
        assert!(out.len() <= 15 * 1024 * 1024, "{} bytes", out.len());
    }
}
