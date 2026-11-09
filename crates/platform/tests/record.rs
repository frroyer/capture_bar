//! US-5.1 AC: recording starts within 300 ms, gives a valid MP4, and drops at most 1% of
//! 1080p60 frame slots (NFR-6). Records the primary display for 10 s.
//!
//! Needs an interactive desktop, so it does not run by default. Move something on screen
//! while it runs (a video, a dragged window): a still screen only measures repeats.
//!   cargo test -p cb-platform --release --test record -- --ignored --nocapture
//!
//! The file holds whatever was on screen; it is deleted at the end.
#![cfg(windows)]

use cb_core::{AudioSources, CaptureTarget, DisplayId};
use std::sync::mpsc;
use std::time::Duration;

#[test]
#[ignore = "needs a desktop session"]
fn records_the_screen_to_a_valid_mp4_in_time() {
    let source = cb_platform::screen_source();
    // As the app does at launch, so the first recording starts as fast as later ones.
    cb_platform::warm_up_recording();
    let dir = std::env::temp_dir().join(format!("cb-record-{}", std::process::id()));
    let (sender, done) = mpsc::channel();
    let handle = source
        .start_recording(
            &CaptureTarget::Display(DisplayId(0)),
            60,
            AudioSources {
                microphone: true,
                system: true,
            },
            &dir.join("ac.mp4"),
            Box::new(move |result| sender.send(result).unwrap()),
        )
        .expect("start");
    std::thread::sleep(Duration::from_secs(10));
    handle.stop();
    let recording = done
        .recv_timeout(Duration::from_secs(10))
        .expect("finished in time")
        .expect("recorded");
    println!("{recording:#?}");

    let head = std::fs::read(&recording.path).expect("file");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(&head[4..8], b"ftyp", "an MP4 file");
    assert!(
        recording.start_latency <= Duration::from_millis(300),
        "{recording:?}"
    );
    let slots = recording.frames + recording.dropped;
    // Ten seconds less the start latency.
    assert!(slots >= 9 * 60, "about 10 s at 60 fps: {recording:?}");
    assert!(
        recording.dropped * 100 <= slots,
        "NFR-6: at most 1% dropped: {recording:?}"
    );
}
