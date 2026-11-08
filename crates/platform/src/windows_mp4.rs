//! H.264 MP4 files through a Media Foundation sink writer (US-5.1), from CPU rows or
//! straight from D3D11 textures. Measured in the US-1.2 spike: docs/spikes/us-1.2-windows.md.

use crate::windows::ensure_mta;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;
use windows::core::{Interface, Result as WinResult, GUID, HSTRING, PWSTR};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoTaskMemFree;

const TICKS_PER_SECOND: i64 = 10_000_000; // Media Foundation time unit: 100 ns.
const MF_VERSION: u32 = (MF_SDK_VERSION << 16) | MF_API_VERSION;

pub(crate) fn ensure_mf() -> WinResult<()> {
    static STARTED: OnceLock<WinResult<()>> = OnceLock::new();
    ensure_mta();
    // SAFETY: called once; Media Foundation stays up for the process lifetime.
    STARTED
        .get_or_init(|| unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) })
        .clone()
}

pub(crate) fn pack(hi: u32, lo: u32) -> u64 {
    (u64::from(hi) << 32) | u64::from(lo)
}

/// H.264 MP4 writer fed with BGRA frames, through a Media Foundation sink writer.
/// The file is fragmented MP4, so a crash leaves the part written so far playable (NFR-12).
pub(crate) struct Mp4Writer {
    writer: IMFSinkWriter,
    stream: u32,
    width: u32,
    height: u32,
    fps: u32,
}

impl Mp4Writer {
    /// `width` and `height` must be even: H.264 uses 4:2:0 chroma.
    /// A device gives hardware encoders a D3D11 device to run on.
    pub fn new(
        path: &Path,
        width: u32,
        height: u32,
        fps: u32,
        device: Option<&ID3D11Device>,
    ) -> WinResult<Self> {
        ensure_mf()?;
        // ~0.1 bit per pixel: enough for screen content at 1080p60 (~12 Mbit/s).
        let bitrate = (u64::from(width) * u64::from(height) * u64::from(fps) / 10) as u32;
        // SAFETY: Media Foundation calls below only read arguments valid for each call.
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 3)?;
            let attrs: IMFAttributes = attrs.expect("MFCreateAttributes without attributes");
            attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
            attrs.SetGUID(
                &MF_TRANSCODE_CONTAINERTYPE,
                &MFTranscodeContainerType_FMPEG4,
            )?;
            if let Some(device) = device {
                let mut token = 0;
                let mut manager = None;
                MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
                let manager: IMFDXGIDeviceManager =
                    manager.expect("MFCreateDXGIDeviceManager without a manager");
                manager.ResetDevice(device, token)?;
                attrs.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &manager)?;
            }
            let url = HSTRING::from(path.as_os_str());
            let writer = MFCreateSinkWriterFromURL(&url, None, &attrs)?;

            let output = MFCreateMediaType()?;
            output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            output.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
            output.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            output.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
            output.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            output.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            // A keyframe every 2 s keeps seeking quick, in players and for previews.
            output.SetUINT32(&MF_MT_MAX_KEYFRAME_SPACING, fps * 2)?;
            let stream = writer.AddStream(&output)?;

            let input = MFCreateMediaType()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            // BGRA, DXGI_FORMAT_B8G8R8A8_UNORM: textures in any other declared format are refused.
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_ARGB32)?;
            input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            input.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
            input.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            // A positive stride means top-down rows, the layout of captured textures.
            input.SetUINT32(&MF_MT_DEFAULT_STRIDE, width * 4)?;
            writer.SetInputMediaType(stream, &input, None)?;

            writer.BeginWriting()?;
            Ok(Self {
                writer,
                stream,
                width,
                height,
                fps,
            })
        }
    }

    /// Queue one BGRA frame at position `index` on the fps timeline.
    /// Blocks when the encoder falls behind, which the spike reports as dropped ticks.
    pub fn write_frame(&self, bgra: &[u8], pitch: usize, index: u64) -> WinResult<()> {
        assert!(bgra.len() >= pitch * self.height as usize);
        // A 2D buffer pads its rows as the encoder expects: with a plain memory buffer,
        // a width that is not a multiple of 16 made the software encoder read past it.
        // SAFETY: the buffer is locked while written, with the pitch it reports, and
        // `bgra` covers `height` rows of `pitch` bytes.
        unsafe {
            let buffer =
                MFCreate2DMediaBuffer(self.width, self.height, MFVideoFormat_ARGB32.data1, false)?;
            let buffer_2d: IMF2DBuffer = buffer.cast()?;
            let (mut scanline, mut stride) = (std::ptr::null_mut(), 0);
            buffer_2d.Lock2D(&mut scanline, &mut stride)?;
            let copied = MFCopyImage(
                scanline,
                stride,
                bgra.as_ptr(),
                pitch as i32,
                self.width * 4,
                self.height,
            );
            buffer_2d.Unlock2D()?;
            copied?;
            buffer.SetCurrentLength(buffer_2d.GetContiguousLength()?)?;
            self.write_buffer(&buffer, index)
        }
    }

    /// Queue a BGRA texture on the writer's device, `width` x `height`, at position `index`.
    /// No CPU copy: the encoder reads the texture, so it must not change afterwards.
    pub fn write_texture(&self, texture: &ID3D11Texture2D, index: u64) -> WinResult<()> {
        // SAFETY: the buffer keeps its own reference to the texture.
        unsafe {
            let buffer = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, texture, 0, false)?;
            // A surface buffer reports no data until its length is set.
            let length = buffer.cast::<IMF2DBuffer>()?.GetContiguousLength()?;
            buffer.SetCurrentLength(length)?;
            self.write_buffer(&buffer, index)
        }
    }

    fn write_buffer(&self, buffer: &IMFMediaBuffer, index: u64) -> WinResult<()> {
        let fps = i64::from(self.fps);
        // SAFETY: the sample takes its own reference to the buffer.
        unsafe {
            let sample = MFCreateSample()?;
            sample.AddBuffer(buffer)?;
            sample.SetSampleTime(index as i64 * TICKS_PER_SECOND / fps)?;
            sample.SetSampleDuration(TICKS_PER_SECOND / fps)?;
            self.writer.WriteSample(self.stream, &sample)
        }
    }

    /// The video encoder the sink writer picked, e.g. "Intel Quick Sync Video H.264 Encoder MFT".
    pub fn encoder_name(&self) -> String {
        let Ok(ex) = self.writer.cast::<IMFSinkWriterEx>() else {
            return "unknown".into();
        };
        for index in 0.. {
            let mut category = GUID::zeroed();
            let mut transform = None;
            // SAFETY: out-pointers are valid; an out-of-range index returns an error.
            let found = unsafe {
                ex.GetTransformForStream(self.stream, index, Some(&mut category), &mut transform)
            };
            if found.is_err() {
                break;
            }
            if category == MFT_CATEGORY_VIDEO_ENCODER {
                return transform
                    .and_then(|t: IMFTransform| encoder_friendly_name(&t))
                    .unwrap_or_else(|| "software (Microsoft H264 Encoder MFT)".into());
            }
        }
        "unknown".into()
    }

    pub fn finish(self) -> WinResult<()> {
        // SAFETY: every queued sample stays owned by the writer until it returns.
        unsafe { self.writer.Finalize() }
    }
}

/// Hardware H.264 encoders this machine offers, by friendly name.
pub(crate) fn hardware_h264_encoders() -> Vec<String> {
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let mut activates = std::ptr::null_mut();
    let mut count = 0;
    // SAFETY: out-pointers are valid; the array and its items are released below.
    unsafe {
        if MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            None,
            Some(&output),
            &mut activates,
            &mut count,
        )
        .is_err()
        {
            return Vec::new();
        }
        let names = std::slice::from_raw_parts_mut(activates, count as usize)
            .iter_mut()
            .filter_map(|slot| {
                let activate: IMFActivate = slot.take()?;
                let mut text = PWSTR::null();
                let mut len = 0;
                activate
                    .GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut text, &mut len)
                    .ok()?;
                let name = text.to_string().ok();
                CoTaskMemFree(Some(text.0 as *const _));
                name
            })
            .collect();
        CoTaskMemFree(Some(activates as *const _));
        names
    }
}

/// Hardware MFTs are asynchronous and carry a device URL; the software one is neither.
fn encoder_friendly_name(transform: &IMFTransform) -> Option<String> {
    // SAFETY: the returned strings are copied, then freed with the COM allocator.
    unsafe {
        let attrs = transform.GetAttributes().ok()?;
        let read = |key: &GUID| {
            let mut text = PWSTR::null();
            let mut len = 0;
            attrs.GetAllocatedString(key, &mut text, &mut len).ok()?;
            let value = text.to_string().ok();
            CoTaskMemFree(Some(text.0 as *const _));
            value
        };
        let is_async = attrs.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0;
        let url = read(&MFT_ENUM_HARDWARE_URL_Attribute);
        let name = read(&MFT_FRIENDLY_NAME_Attribute);
        (is_async || url.is_some()).then(|| name.unwrap_or_else(|| "hardware (unnamed)".into()))
    }
}

/// What a decoder reads back from an MP4: proof the file is valid and not black.
#[derive(Debug)]
pub(crate) struct Mp4Check {
    pub frames: u64,
    pub last_timestamp: Duration,
    pub last_frame_mean_luma: f32,
    /// First and last luma rows of the last frame, averaged: orientation checks.
    pub last_frame_top_luma: f32,
    pub last_frame_bottom_luma: f32,
}

/// `count` frames spread evenly over the video at `path`, first one at the start, each
/// handed to `each` as soon as it is decoded.
pub(crate) fn video_frames(
    path: &Path,
    count: u32,
    each: &mut dyn FnMut(cb_core::Frame),
) -> WinResult<()> {
    ensure_mf()?;
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    // SAFETY: Media Foundation calls below only read arguments valid for each call;
    // the locked slice is only used before Unlock.
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs: IMFAttributes = attrs.expect("MFCreateAttributes without attributes");
        // Lets the reader convert the decoded YUV to RGB.
        attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &attrs)?;
        let rgb = MFCreateMediaType()?;
        rgb.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        rgb.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        reader.SetCurrentMediaType(stream, None, &rgb)?;
        let size = reader
            .GetCurrentMediaType(stream)?
            .GetUINT64(&MF_MT_FRAME_SIZE)?;
        let (width, height) = ((size >> 32) as usize, (size & 0xffff_ffff) as usize);
        let duration = u64::try_from(
            &reader
                .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)?,
        )?;

        for i in 0..u64::from(count) {
            let target = duration * i / u64::from(count);
            reader.SetCurrentPosition(
                &GUID::zeroed(),
                &windows::Win32::System::Com::StructuredStorage::PROPVARIANT::from(target as i64),
            )?;
            // A seek lands on the keyframe before `target`: decode forward to it.
            let sample = loop {
                let mut flags = 0u32;
                let mut timestamp = 0i64;
                let mut sample = None;
                reader.ReadSample(
                    stream,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut timestamp),
                    Some(&mut sample),
                )?;
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break None;
                }
                if sample.is_some() && timestamp as u64 >= target {
                    break sample;
                }
            };
            let Some(sample): Option<IMFSample> = sample else {
                break;
            };
            // The decoder pads rows to its block size, so the pitch is wider than the
            // picture unless the width is a multiple of 16. The media type only gives the
            // real stride once a frame is decoded; negative means bottom-up rows.
            let stride = reader
                .GetCurrentMediaType(stream)?
                .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                .map_or(width as i32 * 4, |s| s as i32);
            let pitch = stride.unsigned_abs() as usize;
            let buffer = sample.ConvertToContiguousBuffer()?;
            let mut data = std::ptr::null_mut();
            let mut len = 0;
            buffer.Lock(&mut data, None, Some(&mut len))?;
            let bytes = std::slice::from_raw_parts(data as *const u8, len as usize);
            let mut rgba = Vec::with_capacity(width * height * 4);
            if pitch >= width * 4 && bytes.len() >= pitch * height {
                for y in 0..height {
                    let row = if stride < 0 { height - 1 - y } else { y } * pitch;
                    for [b, g, r, _] in bytes[row..row + width * 4].as_chunks::<4>().0 {
                        rgba.extend_from_slice(&[*r, *g, *b, 255]);
                    }
                }
            }
            buffer.Unlock()?;
            if !rgba.is_empty() {
                each(cb_core::Frame {
                    width: width as u32,
                    height: height as u32,
                    rgba,
                    captured_at: std::time::SystemTime::now(),
                });
            }
        }
        Ok(())
    }
}

pub(crate) fn decode_check(path: &Path, width: u32, height: u32) -> WinResult<Mp4Check> {
    ensure_mf()?;
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    // SAFETY: Media Foundation calls below only read arguments valid for each call;
    // the locked slice is only used before Unlock.
    unsafe {
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None)?;
        let nv12 = MFCreateMediaType()?;
        nv12.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        nv12.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        reader.SetCurrentMediaType(stream, None, &nv12)?;
        let mut check = Mp4Check {
            frames: 0,
            last_timestamp: Duration::ZERO,
            last_frame_mean_luma: 0.0,
            last_frame_top_luma: 0.0,
            last_frame_bottom_luma: 0.0,
        };
        let mean =
            |rows: &[u8]| rows.iter().map(|&y| f32::from(y)).sum::<f32>() / rows.len() as f32;
        loop {
            let mut flags = 0u32;
            let mut timestamp = 0i64;
            let mut sample = None;
            reader.ReadSample(
                stream,
                0,
                None,
                Some(&mut flags),
                Some(&mut timestamp),
                Some(&mut sample),
            )?;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
            let Some(sample): Option<IMFSample> = sample else {
                continue;
            };
            check.frames += 1;
            check.last_timestamp = Duration::from_nanos(timestamp as u64 * 100);

            // Known once a frame is decoded: rows are padded to the decoder's block size.
            let pitch = reader
                .GetCurrentMediaType(stream)?
                .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                .unwrap_or(width) as usize;
            let buffer = sample.ConvertToContiguousBuffer()?;
            let mut data = std::ptr::null_mut();
            let mut len = 0;
            buffer.Lock(&mut data, None, Some(&mut len))?;
            let bytes = std::slice::from_raw_parts(data as *const u8, len as usize);
            let (w, h) = (width as usize, height as usize);
            if bytes.len() >= pitch * h {
                check.last_frame_top_luma = mean(&bytes[..w]);
                check.last_frame_bottom_luma = mean(&bytes[pitch * (h - 1)..pitch * (h - 1) + w]);
                // Every 16th row is plenty for a black-frame check.
                let sum: f32 = (0..h)
                    .step_by(16)
                    .map(|y| mean(&bytes[pitch * y..pitch * y + w]))
                    .sum();
                check.last_frame_mean_luma = sum / h.div_ceil(16) as f32;
            }
            buffer.Unlock()?;
        }
        Ok(check)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 640;
    const H: u32 = 480;
    const FPS: u32 = 30;

    /// White over black: a decoded frame shows whether it is upside down.
    fn half_white() -> Vec<u8> {
        let mut bgra = vec![255u8; (W * H * 4) as usize];
        bgra[(W * 4 * H / 2) as usize..].fill(0);
        bgra
    }

    fn temp_mp4(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cb-mp4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn assert_white_over_black(check: &Mp4Check, frames: u64) {
        assert_eq!(check.frames, frames, "{check:?}");
        // Decoded times are whole 100 ns ticks, so allow for the cut-off fraction.
        let last = Duration::from_secs(frames - 1) / FPS - Duration::from_micros(1);
        assert!(check.last_timestamp >= last, "{check:?}");
        assert!(check.last_frame_top_luma > 200.0, "{check:?}");
        assert!(check.last_frame_bottom_luma < 50.0, "{check:?}");
    }

    /// Encode from CPU rows, decode, and check frame count, timing and orientation.
    /// Needs no screen.
    #[test]
    fn mp4_round_trip_keeps_frames_and_orientation() {
        let path = temp_mp4("rows.mp4");
        let mp4 = match Mp4Writer::new(&path, W, H, FPS, None) {
            Ok(mp4) => mp4,
            Err(e) => {
                // Some Windows Server images ship no H.264 encoder.
                eprintln!("skipped: no H.264 encoder on this machine: {e}");
                return;
            }
        };
        let bgra = half_white();
        for i in 0..30 {
            mp4.write_frame(&bgra, (W * 4) as usize, i).unwrap();
        }
        mp4.finish().unwrap();
        let check = decode_check(&path, W, H).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_white_over_black(&check, 30);
    }

    /// The recording path: GPU textures straight to the encoder (US-5.1). Needs a GPU.
    #[test]
    fn mp4_from_textures_keeps_frames_and_orientation() {
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_BIND_SHADER_RESOURCE, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC,
            D3D11_USAGE_DEFAULT,
        };
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
        };
        let Ok(gpu) = crate::windows::shared_gpu() else {
            eprintln!("skipped: no hardware D3D11 device");
            return;
        };
        let bgra = half_white();
        let desc = D3D11_TEXTURE2D_DESC {
            Width: W,
            Height: H,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let data = D3D11_SUBRESOURCE_DATA {
            pSysMem: bgra.as_ptr().cast(),
            SysMemPitch: W * 4,
            SysMemSlicePitch: 0,
        };
        let mut texture = None;
        // SAFETY: `desc`, `data` and the out-pointer are valid for the call.
        unsafe {
            gpu.device
                .CreateTexture2D(&desc, Some(&data), Some(&mut texture))
                .unwrap();
        }
        let texture = texture.unwrap();

        let path = temp_mp4("textures.mp4");
        let mp4 = Mp4Writer::new(&path, W, H, FPS, Some(&gpu.device)).unwrap();
        for i in 0..30 {
            mp4.write_texture(&texture, i).unwrap();
        }
        mp4.finish().unwrap();
        let check = decode_check(&path, W, H).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_white_over_black(&check, 30);
    }

    /// The hover preview: frames spread over the video, the right way up and straight.
    /// 650 x 482 is not a multiple of 16, so the decoder pads its rows: reading them at
    /// the picture's width shears the image (seen on a region recording).
    #[test]
    fn video_frames_spread_over_the_video() {
        let (w, h) = (650u32, 482u32);
        let path = temp_mp4("preview.mp4");
        let Ok(mp4) = Mp4Writer::new(&path, w, h, FPS, None) else {
            eprintln!("skipped: no H.264 encoder on this machine");
            return;
        };
        // White on the left, black on the right.
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        for row in bgra.chunks_exact_mut((w * 4) as usize) {
            row[..(w * 2) as usize].fill(255);
        }
        for i in 0..90 {
            mp4.write_frame(&bgra, (w * 4) as usize, i).unwrap();
        }
        mp4.finish().unwrap();
        let mut frames = Vec::new();
        video_frames(&path, 6, &mut |frame| frames.push(frame)).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(frames.len(), 6);
        let frame = &frames[5];
        assert_eq!((frame.width, frame.height), (w, h));
        let red_at = |x: u32, y: u32| frame.rgba[((y * w + x) * 4) as usize];
        for y in [2, h / 2, h - 3] {
            assert!(red_at(20, y) > 200, "white on the left of row {y}");
            assert!(red_at(w - 20, y) < 50, "black on the right of row {y}");
        }
    }

    /// NFR-12: a recording cut off before `finish` (a crash) still plays up to
    /// its last complete fragment.
    #[test]
    fn unfinished_mp4_still_plays() {
        let path = temp_mp4("crashed.mp4");
        let Ok(mp4) = Mp4Writer::new(&path, W, H, FPS, None) else {
            eprintln!("skipped: no H.264 encoder on this machine");
            return;
        };
        let bgra = half_white();
        for i in 0..90 {
            mp4.write_frame(&bgra, (W * 4) as usize, i).unwrap();
        }
        // Never finalised, as after a crash.
        std::mem::forget(mp4);
        let check = decode_check(&path, W, H);
        eprintln!("{check:?}");
        let check = check.unwrap();
        assert!(check.frames > 0, "{check:?}");
    }
}
