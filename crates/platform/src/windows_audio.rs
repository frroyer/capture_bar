//! Microphone and system sound on Windows through WASAPI (US-5.3): the default capture
//! device, and the default playback device in loopback. Both are asked for 48 kHz 16-bit
//! stereo, which Windows converts to. Packets carry the time they were captured, so
//! [`Mixer`] puts them on the same timeline as the video.

use crate::audio_mix::{Mixer, CHANNELS, RATE};
use crate::windows_record::Control;
use cb_core::AudioSources;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::core::Result as WinResult;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

/// The mix is handed out this far behind now, so packets still in a device's buffer
/// (one 10 ms period, plus scheduling) land before their place is gone.
const LATENCY: i64 = RATE as i64 / 10;
const POLL: Duration = Duration::from_millis(10);
/// Device buffer: room for a poll that runs late.
const BUFFER_100NS: i64 = 2_000_000;

/// 16-bit stereo at [`RATE`], from its first frame on the recording's timeline.
pub(crate) type Chunk = (i64, Vec<i16>);

pub(crate) struct AudioCapture {
    pub chunks: Receiver<Chunk>,
    thread: JoinHandle<()>,
}

impl AudioCapture {
    /// Wait for the last chunk, once the recording is stopping.
    pub fn join(self) -> Receiver<Chunk> {
        let _ = self.thread.join();
        self.chunks
    }
}

struct Source {
    name: &'static str,
    microphone: bool,
    // Kept alive: capture stops with the client.
    _client: IAudioClient,
    capture: IAudioCaptureClient,
}

/// Open the sources asked for and mix them, on a thread of their own. Opening a device
/// can take hundreds of milliseconds, so the recording does not wait: the sound track
/// is silent until the devices run, and stays silent if none can be opened.
pub(crate) fn start(sources: AudioSources, control: Arc<Control>) -> AudioCapture {
    let (out, chunks) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("recording audio".into())
        .spawn(move || run(open_all(sources), &control, &out))
        .expect("spawn audio thread");
    AudioCapture { chunks, thread }
}

fn open_all(sources: AudioSources) -> Vec<Source> {
    crate::windows::ensure_mta();
    let started = Instant::now();
    let wanted = [
        (sources.microphone, "microphone", eCapture, false),
        (sources.system, "system sound", eRender, true),
    ];
    let opened = wanted
        .into_iter()
        .filter(|(on, ..)| *on)
        .filter_map(|(_, name, flow, loopback)| match open(flow, loopback) {
            Ok((client, capture)) => Some(Source {
                name,
                microphone: !loopback,
                _client: client,
                capture,
            }),
            Err(e) => {
                tracing::warn!("recording without the {name}: {e}");
                None
            }
        })
        .collect();
    tracing::debug!(
        ms = started.elapsed().as_millis() as u64,
        "sound devices opened"
    );
    opened
}

fn open(flow: EDataFlow, loopback: bool) -> WinResult<(IAudioClient, IAudioCaptureClient)> {
    let block = (CHANNELS * 2) as u16;
    let format = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: CHANNELS as u16,
        nSamplesPerSec: RATE,
        nAvgBytesPerSec: RATE * u32::from(block),
        nBlockAlign: block,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    let mut flags = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    if loopback {
        flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
    }
    // SAFETY: COM calls with arguments valid for each call; `format` outlives Initialize.
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = devices.GetDefaultAudioEndpoint(flow, eConsole)?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            flags,
            BUFFER_100NS,
            0,
            &format,
            None,
        )?;
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        Ok((client, capture))
    }
}

/// The performance counter in 100 ns units, the unit of WASAPI packet times.
fn qpc_100ns() -> u64 {
    let (mut count, mut freq) = (0i64, 0i64);
    // SAFETY: out-pointers are valid; both calls cannot fail on Windows XP and later.
    unsafe {
        let _ = QueryPerformanceCounter(&mut count);
        let _ = QueryPerformanceFrequency(&mut freq);
    }
    (i128::from(count) * 10_000_000 / i128::from(freq.max(1))) as u64
}

fn run(sources: Vec<Source>, control: &Control, out: &Sender<Chunk>) {
    let mut sources: Vec<Option<Source>> = sources.into_iter().map(Some).collect();
    let mut mixer = Mixer::new(sources.len());
    let mut level = 0.0f32;
    loop {
        let stopping = control.stop.load(Relaxed);
        let mut mic = None;
        for (index, slot) in sources.iter_mut().enumerate() {
            let Some(source) = slot else { continue };
            match read(source, index, control, &mut mixer) {
                Ok(peak) if source.microphone => mic = Some(peak),
                Ok(_) => {}
                Err(e) => {
                    // Unplugged, or the default device changed: the rest has no sound from it.
                    tracing::warn!("{} stopped: {e}", source.name);
                    *slot = None;
                }
            }
        }
        // A meter's fall: a peak fades to a third in 100 ms.
        level = mic.map_or(-1.0, |peak| peak.max(level.max(0.0) * 0.9));
        control.mic_level.store(level.to_bits(), Relaxed);
        // Nothing is handed out before the first frame.
        let until = control.frames_now().map(|now| {
            if stopping {
                now.max(mixer.end())
            } else {
                now - LATENCY
            }
        });
        if let Some(chunk) = until.and_then(|until| mixer.take(until)) {
            if out.send(chunk).is_err() {
                return;
            }
        }
        if stopping {
            return;
        }
        std::thread::sleep(POLL);
    }
}

/// Move every packet waiting in `source` into the mixer; returns their peak, from 0 to 1.
fn read(source: &Source, index: usize, control: &Control, mixer: &mut Mixer) -> WinResult<f32> {
    let mut peak = 0u16;
    // SAFETY: the buffer from GetBuffer holds `frames` frames of 16-bit stereo and is
    // only read before ReleaseBuffer.
    unsafe {
        while source.capture.GetNextPacketSize()? > 0 {
            let (mut data, mut frames, mut flags, mut captured) = (std::ptr::null_mut(), 0, 0, 0);
            source.capture.GetBuffer(
                &mut data,
                &mut frames,
                &mut flags,
                None,
                Some(&mut captured),
            )?;
            let (now, now_qpc) = (Instant::now(), qpc_100ns());
            let age = if captured > 0 {
                Duration::from_nanos(now_qpc.saturating_sub(captured) * 100)
            } else {
                Duration::from_secs(u64::from(frames)) / RATE
            };
            let at = now.checked_sub(age).and_then(|at| control.frames_at(at));
            if let Some(at) = at {
                let len = frames as usize * CHANNELS;
                let pcm = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                    vec![0; len]
                } else {
                    std::slice::from_raw_parts(data, len * 2)
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|&b| i16::from_le_bytes(b))
                        .collect()
                };
                peak = pcm.iter().fold(peak, |max, s| max.max(s.unsigned_abs()));
                // Gaps are normal (a quiet output sends nothing in loopback); a jump back,
                // or flagged timestamps, are how a track would slip (US-10.2).
                let ms = |frames: i64| frames * 1000 / i64::from(RATE);
                if let Some(jump) = mixer.add(index, at, &pcm) {
                    tracing::debug!(source = source.name, jump_ms = ms(jump), "sound re-synced");
                }
                if flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 != 0 {
                    tracing::warn!(source = source.name, "sound packet with a bad timestamp");
                }
            }
            source.capture.ReleaseBuffer(frames)?;
        }
    }
    Ok(f32::from(peak) / f32::from(i16::MAX))
}

/// How long the clap test's beep lasts.
const BEEP_MS: u32 = 120;

/// Play the clap test's beep (US-10.2) on the default output, on a thread of its own, and
/// tell `played` when its first sample played, by the device's clock.
pub(crate) fn beep(played: Box<dyn FnOnce(Option<Instant>) + Send>) {
    std::thread::Builder::new()
        .name("clap beep".into())
        .spawn(move || {
            crate::windows::ensure_mta();
            let at = play_beep()
                .inspect_err(|e| tracing::warn!("clap beep: {e}"))
                .ok();
            played(at);
        })
        .expect("spawn clap beep thread");
}

/// Play the beep and return when it started playing. The stream is new each time, so its
/// position 0 is the beep's first sample: the device clock says when that played.
fn play_beep() -> WinResult<Instant> {
    let block = (CHANNELS * 2) as u16;
    let format = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: CHANNELS as u16,
        nSamplesPerSec: RATE,
        nAvgBytesPerSec: RATE * u32::from(block),
        nBlockAlign: block,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    let tone = tone(RATE, 1000.0, BEEP_MS, 0.5);
    // SAFETY: COM calls with arguments valid for each call; `format` outlives Initialize;
    // the buffer from GetBuffer holds `frames` frames and is only written before
    // ReleaseBuffer.
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = devices.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
            BUFFER_100NS,
            0,
            &format,
            None,
        )?;
        let render: IAudioRenderClient = client.GetService()?;
        let clock: IAudioClock = client.GetService()?;
        let frames = (tone.len() as u32).min(client.GetBufferSize()?);
        let data = render.GetBuffer(frames)?;
        let out = std::slice::from_raw_parts_mut(data.cast::<i16>(), frames as usize * CHANNELS);
        for (frame, &v) in out.as_chunks_mut::<CHANNELS>().0.iter_mut().zip(&tone) {
            frame.fill(v);
        }
        render.ReleaseBuffer(frames, 0)?;
        let frequency = clock.GetFrequency()?.max(1);
        client.Start()?;
        let deadline = Instant::now() + Duration::from_secs(1);
        let started = loop {
            let (mut position, mut qpc) = (0u64, 0u64);
            clock.GetPosition(&mut position, Some(&mut qpc))?;
            if position > 0 {
                // `position` had played at `qpc`: the first sample, that long before.
                let (now, now_qpc) = (Instant::now(), qpc_100ns());
                let since_qpc = Duration::from_nanos(now_qpc.saturating_sub(qpc) * 100);
                let played = Duration::from_secs_f64(position as f64 / frequency as f64);
                break now.checked_sub(since_qpc + played);
            }
            if Instant::now() > deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        std::thread::sleep(Duration::from_millis(u64::from(BEEP_MS) + 50));
        let _ = client.Stop();
        started.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))
    }
}

/// `ms` of a `freq` Hz tone at `rate`, mono, at `gain` (0–1), with 3 ms ramps so it
/// does not click.
fn tone(rate: u32, freq: f32, ms: u32, gain: f32) -> Vec<i16> {
    let frames = rate * ms / 1000;
    let ramp = (rate * 3 / 1000).max(1) as f32;
    (0..frames)
        .map(|i| {
            let edge = (i.min(frames - 1 - i) as f32 / ramp).min(1.0);
            let t = i as f32 / rate as f32;
            ((t * freq * std::f32::consts::TAU).sin() * gain * edge * 32767.0) as i16
        })
        .collect()
}

#[cfg(test)]
mod beep_tests {
    use super::*;

    #[test]
    fn the_beep_ramps_in_and_out() {
        let tone = tone(48_000, 1000.0, 120, 0.5);
        assert_eq!(tone.len(), 48_000 * 120 / 1000);
        assert_eq!((tone[0], tone[tone.len() - 1]), (0, 0));
        let loudest = tone.iter().map(|v| v.unsigned_abs()).max().unwrap();
        assert!((15_000..=16_500).contains(&loudest), "{loudest}");
    }
}
