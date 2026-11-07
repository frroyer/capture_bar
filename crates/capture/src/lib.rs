//! Capture service (US-4.x). Requests are queued and executed on a worker
//! thread so the shortcut handler returns immediately (FR-20, NFR-4).

use cb_core::{CaptureError, CaptureTarget, Frame, ScreenSource};
use crossbeam_channel::{unbounded, Receiver, Sender};
use std::sync::Arc;
use std::thread;

#[derive(Debug)]
pub enum CaptureEvent {
    Still {
        request_id: u64,
        frame: Frame,
    },
    Failed {
        request_id: u64,
        error: CaptureError,
    },
}

struct Request {
    id: u64,
    target: CaptureTarget,
}

pub struct CaptureService {
    tx: Sender<Request>,
    events: Receiver<CaptureEvent>,
    next_id: u64,
}

impl CaptureService {
    /// `wake` runs after each finished request, to get the UI to read the result.
    pub fn new(source: Arc<dyn ScreenSource>, wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = unbounded::<Request>();
        let (ev_tx, events) = unbounded::<CaptureEvent>();
        thread::Builder::new()
            .name("capture".into())
            .spawn(move || {
                for req in rx {
                    let ev = match source.grab(&req.target) {
                        Ok(frame) => CaptureEvent::Still {
                            request_id: req.id,
                            frame,
                        },
                        Err(error) => CaptureEvent::Failed {
                            request_id: req.id,
                            error,
                        },
                    };
                    if ev_tx.send(ev).is_err() {
                        break;
                    }
                    wake();
                }
            })
            .expect("spawn capture thread");
        Self {
            tx,
            events,
            next_id: 0,
        }
    }

    /// Queue a still capture. Never blocks.
    pub fn request_still(&mut self, target: CaptureTarget) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        if self.tx.send(Request { id, target }).is_err() {
            tracing::error!("capture thread is gone");
        }
        id
    }

    /// Drain finished captures (call from the UI loop).
    pub fn poll(&self) -> impl Iterator<Item = CaptureEvent> + '_ {
        self.events.try_iter()
    }
}
