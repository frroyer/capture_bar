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
    loop {
        let stopping = control.stop.load(Relaxed);
        for (index, slot) in sources.iter_mut().enumerate() {
            let Some(source) = slot else { continue };
            if let Err(e) = read(source, index, control, &mut mixer) {
                // Unplugged, or the default device changed: the rest has no sound from it.
                tracing::warn!("{} stopped: {e}", source.name);
                *slot = None;
            }
        }
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

/// Move every packet waiting in `source` into the mixer.
fn read(source: &Source, index: usize, control: &Control, mixer: &mut Mixer) -> WinResult<()> {
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
                mixer.add(index, at, &pcm);
            }
            source.capture.ReleaseBuffer(frames)?;
        }
    }
    Ok(())
}
