//! A recording played in Quick Look: decoded on its own thread, scaled down, and handed
//! to the UI frame by frame on time. It loops, and stops when dropped.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use cb_core::Frame;

/// Smooth enough to see what happens, light enough to decode faster than it plays.
const FPS: u32 = 15;
/// Frames are scaled down to fit in this, in pixels: the window is no bigger.
const MAX_SIZE: (u32, u32) = (1600, 1000);
/// Frames decoded ahead of time.
const AHEAD: usize = 8;

pub struct PeekVideo {
    path: PathBuf,
    frames: mpsc::Receiver<Frame>,
    stop: Arc<AtomicBool>,
    started: Instant,
    /// Frames shown since `started`.
    shown: u32,
    next: Option<Frame>,
}

impl PeekVideo {
    pub fn start(path: &Path, wake: impl Fn() + Send + 'static) -> Self {
        let (tx, frames) = mpsc::sync_channel(AHEAD);
        let stop = Arc::new(AtomicBool::new(false));
        let (from, stopped) = (path.to_path_buf(), stop.clone());
        let spawned = std::thread::Builder::new()
            .name("peek video".into())
            .spawn(move || {
                let result = cb_platform::video_clip(&from, FPS, &mut |frame, _| {
                    let small =
                        cb_encode::thumbnail(&frame, MAX_SIZE.0, MAX_SIZE.1).unwrap_or(frame);
                    // Blocks while the UI is AHEAD frames behind; fails once it is gone.
                    let sent = tx.send(small).is_ok();
                    wake();
                    sent && !stopped.load(Ordering::Relaxed)
                });
                if let Err(error) = result {
                    tracing::warn!(%error, ?from, "no Quick Look video");
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "no Quick Look video thread");
        }
        Self {
            path: path.to_path_buf(),
            frames,
            stop,
            started: Instant::now(),
            shown: 0,
            next: None,
        }
    }

    /// The frame due now, if a new one is. Starts again at the end; `None` for good once
    /// the video turned out unreadable.
    pub fn poll(&mut self, wake: impl Fn() + Send + 'static) -> Option<Option<Frame>> {
        let mut due = None;
        loop {
            if self.next.is_none() {
                match self.frames.try_recv() {
                    Ok(frame) => {
                        // Time runs from the first frame: opening the file takes a while.
                        if self.shown == 0 {
                            self.started = Instant::now();
                        }
                        self.next = Some(frame);
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) if self.shown == 0 && due.is_none() => {
                        return Some(None);
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        *self = Self::start(&self.path.clone(), wake);
                        break;
                    }
                }
            }
            let at = self.started + Duration::from_secs(u64::from(self.shown)) / FPS;
            if Instant::now() < at {
                break;
            }
            // Behind: skip to the newest frame that is due.
            due = self.next.take();
            self.shown += 1;
        }
        due.map(Some)
    }

    /// How long until the next frame is due.
    pub fn wait(&self) -> Duration {
        let at = self.started + Duration::from_secs(u64::from(self.shown)) / FPS;
        at.saturating_duration_since(Instant::now())
            .max(Duration::from_millis(5))
    }
}

impl Drop for PeekVideo {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
