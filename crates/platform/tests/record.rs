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

/// US-5.4: with the camera on and a bubble placed, the recording shows the camera
/// there. The file stays in the temp folder for a look at a frame.
#[test]
#[ignore = "needs a desktop session and a camera"]
fn records_the_camera_bubble() {
    cb_platform::set_camera(true);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    // A camera's first pictures are black while it sets its exposure.
    let lit = |frame: &cb_core::Frame| frame.rgba.as_chunks::<4>().0.iter().any(|p| p[0] > 60);
    while !cb_platform::camera_frame(None, 1).is_some_and(|(_, frame)| lit(&frame)) {
        assert!(
            std::time::Instant::now() < deadline,
            "no lit camera picture"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // Bottom-right of the primary display, 300 px wide.
    let display = cb_platform::screen_source().displays().expect("displays")[0].clone();
    let b = display.bounds;
    let bubble = cb_core::Rect {
        x: b.x + b.width as i32 - 340,
        y: b.y + b.height as i32 - 340,
        width: 300,
        height: 300,
    };
    cb_platform::set_camera_bubble(Some((bubble, cb_core::BubbleShape::Circle)));
    let path = std::env::temp_dir().join("cb-bubble.mp4");
    let (sender, done) = mpsc::channel();
    let handle = cb_platform::screen_source()
        .start_recording(
            &CaptureTarget::Display(DisplayId(0)),
            30,
            AudioSources::default(),
            &path,
            Box::new(move |result| sender.send(result).unwrap()),
        )
        .expect("start");
    std::thread::sleep(Duration::from_secs(3));
    handle.stop();
    let recording = done
        .recv_timeout(Duration::from_secs(10))
        .expect("finished in time")
        .expect("recorded");
    cb_platform::set_camera_bubble(None);
    cb_platform::set_camera(false);
    println!("{recording:#?} bubble {bubble:?}");
    assert_eq!(recording.dropped, 0, "{recording:?}");
}

/// US-5.5 AC: a capture during a recording gives a picture of what is recorded, at its
/// size, and the video loses no frame for it. Five stills over 3 s, one while paused.
#[test]
#[ignore = "needs a desktop session"]
fn stills_while_recording_cost_no_frame() {
    cb_platform::warm_up_recording();
    let display = cb_platform::screen_source().displays().expect("displays")[0].clone();
    let path = std::env::temp_dir().join("cb-stills.mp4");
    let (sender, done) = mpsc::channel();
    let mut handle = cb_platform::screen_source()
        .start_recording(
            &CaptureTarget::Display(display.id),
            30,
            AudioSources::default(),
            &path,
            Box::new(move |result| sender.send(result).unwrap()),
        )
        .expect("start");
    let (still_sender, stills) = mpsc::channel();
    for n in 0..5 {
        std::thread::sleep(Duration::from_millis(600));
        if n == 3 {
            handle.pause();
        }
        let sender = still_sender.clone();
        handle.still(Box::new(move |frame| sender.send(frame).unwrap()));
        if n == 3 {
            std::thread::sleep(Duration::from_millis(300));
            handle.resume();
        }
    }
    let frames: Vec<_> = (0..5)
        .map(|_| {
            stills
                .recv_timeout(Duration::from_secs(2))
                .expect("still in time")
                .expect("still")
        })
        .collect();
    handle.stop();
    let recording = done
        .recv_timeout(Duration::from_secs(10))
        .expect("finished in time")
        .expect("recorded");
    let _ = std::fs::remove_file(&recording.path);
    println!("{recording:#?}");
    for frame in frames {
        // The recorded picture, at its size.
        assert_eq!(
            (frame.width, frame.height),
            (recording.width, recording.height)
        );
        assert_eq!(frame.rgba.len(), (frame.width * frame.height * 4) as usize);
        assert!(frame
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p != &[0, 0, 0, 255]));
    }
    assert_eq!(recording.dropped, 0, "{recording:?}");
}

/// The largest step between two neighbouring pixels of `frame`, on any channel: sharp
/// text gives ~200, a 32 px blur a few.
fn sharpest_edge(frame: &cb_core::Frame) -> u8 {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let at = |x: usize, y: usize, c: usize| frame.rgba[(y * w + x) * 4 + c];
    (0..h)
        .flat_map(|y| (1..w).map(move |x| (x, y)))
        .flat_map(|(x, y)| (0..3).map(move |c| at(x, y, c).abs_diff(at(x - 1, y, c))))
        .max()
        .unwrap_or(0)
}

/// US-10.5 AC: a masked area is blurred in the video and in stills. Masks the whole
/// display, so no sharp edge may remain anywhere.
#[test]
#[ignore = "needs a desktop session"]
fn masked_areas_are_blurred_in_the_video_and_stills() {
    cb_platform::warm_up_recording();
    let display = cb_platform::screen_source().displays().expect("displays")[0].clone();
    // Larger than any display: a test process is not DPI aware, so its display bounds
    // may be in scaled pixels.
    let everything = cb_core::Rect {
        x: display.bounds.x,
        y: display.bounds.y,
        width: 20_000,
        height: 20_000,
    };
    cb_platform::set_masks(vec![cb_core::Mask::Area(everything)]);
    let path = std::env::temp_dir().join("cb-masked.mp4");
    let (sender, done) = mpsc::channel();
    let handle = cb_platform::screen_source()
        .start_recording(
            &CaptureTarget::Display(display.id),
            30,
            AudioSources::default(),
            &path,
            Box::new(move |result| sender.send(result).unwrap()),
        )
        .expect("start");
    std::thread::sleep(Duration::from_millis(800));
    let (still_sender, still) = mpsc::channel();
    handle.still(Box::new(move |frame| still_sender.send(frame).unwrap()));
    let still = still
        .recv_timeout(Duration::from_secs(2))
        .expect("still in time")
        .expect("still");
    std::thread::sleep(Duration::from_millis(500));
    handle.stop();
    let recording = done
        .recv_timeout(Duration::from_secs(10))
        .expect("finished in time")
        .expect("recorded");
    cb_platform::set_masks(Vec::new());
    let mut frames = Vec::new();
    cb_platform::video_frames(&recording.path, 3, &mut |f| frames.push(f)).expect("decode");
    let _ = std::fs::remove_file(&recording.path);
    assert!(!frames.is_empty());
    // H.264 adds a little ringing of its own.
    for frame in frames.iter().chain([&still]) {
        let edge = sharpest_edge(frame);
        println!("{} x {}: sharpest edge {edge}", frame.width, frame.height);
        assert!(edge < 40, "a sharp edge left: {edge}");
    }
}

/// US-10.6: with the camera on, the webcam gets its own video beside the screen's, on
/// the same clock.
#[test]
#[ignore = "needs a desktop session and a camera"]
fn the_webcam_gets_its_own_track() {
    cb_platform::warm_up_recording();
    cb_platform::set_camera(true);
    let dir = std::env::temp_dir().join(format!("cb-webcam-{}", std::process::id()));
    let webcam = dir.join("webcam.mp4");
    cb_platform::set_webcam_track(Some(webcam.clone()));
    let (sender, done) = mpsc::channel();
    let handle = cb_platform::screen_source()
        .start_recording(
            &CaptureTarget::Display(DisplayId(0)),
            30,
            AudioSources::default(),
            &dir.join("screen.mp4"),
            Box::new(move |result| sender.send(result).unwrap()),
        )
        .expect("start");
    std::thread::sleep(Duration::from_secs(4));
    handle.stop();
    let recording = done
        .recv_timeout(Duration::from_secs(10))
        .expect("finished in time")
        .expect("recorded");
    cb_platform::set_camera(false);
    println!("{recording:#?}");
    let track = recording.webcam.expect("a webcam track");
    assert_eq!(track.path, webcam);
    assert!(track.offset < Duration::from_secs(3), "{track:?}");
    let mut frames = 0;
    cb_platform::video_frames(&webcam, 2, &mut |f| {
        assert_eq!((f.width, f.height), (track.width, track.height));
        frames += 1;
    })
    .expect("decode the webcam track");
    assert!(frames > 0);
    let faces = cb_platform::face_boxes(&webcam).expect("face detection runs");
    println!("faces: {faces:?}");
    assert!(!faces.is_empty(), "one entry a second");
    let _ = std::fs::remove_dir_all(&dir);
}
