//! US-1.2 spike runner: saves one PNG and records one MP4, then prints the
//! numbers to log against NFR-4 and NFR-6.
//!
//!   cargo run -p cb-platform --example spike --release -- <out-dir> [display-index] [seconds] [fps]
//!
//! The files contain whatever is on screen. Don't commit them.

use cb_core::CaptureTarget;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out_dir = args
        .first()
        .ok_or("usage: spike <out-dir> [display-index] [seconds] [fps]")?;
    let index: usize = args.get(1).map_or(Ok(0), |a| a.parse())?;
    let seconds: u32 = args.get(2).map_or(Ok(10), |a| a.parse())?;
    let fps: u32 = args.get(3).map_or(Ok(60), |a| a.parse())?;
    std::fs::create_dir_all(out_dir)?;

    println!("backend: {}", cb_platform::backend_name());
    let source = cb_platform::screen_source();
    let displays = source.displays()?;
    for (i, d) in displays.iter().enumerate() {
        println!(
            "display {i}: {} {:?} scale {}",
            d.name, d.bounds, d.scale_factor
        );
    }
    let display = displays.get(index).ok_or("no such display")?;
    println!("using display {index}");

    // NFR-4: shortcut -> item in buffer <= 100 ms. The grab is the OS-dependent part.
    let target = CaptureTarget::Display(display.id);
    let mut grabs_ms = Vec::new();
    let mut frame = source.grab(&target)?;
    for _ in 0..20 {
        let start = Instant::now();
        frame = source.grab(&target)?;
        grabs_ms.push(start.elapsed().as_secs_f32() * 1000.0);
    }
    grabs_ms.sort_by(f32::total_cmp);
    println!(
        "grab ({}x{}), 20 runs: min {:.1} ms, median {:.1} ms, max {:.1} ms",
        frame.width,
        frame.height,
        grabs_ms[0],
        grabs_ms[grabs_ms.len() / 2],
        grabs_ms[grabs_ms.len() - 1]
    );

    let start = Instant::now();
    let png_path = std::path::Path::new(out_dir).join("spike.png");
    cb_encode::write_png(
        &frame,
        std::io::BufWriter::new(std::fs::File::create(&png_path)?),
    )?;
    let luma = frame
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| f64::from(p[1]))
        .sum::<f64>()
        / (frame.rgba.len() / 4) as f64;
    println!(
        "png: {:.1} ms on the worker, {} bytes, mean green {luma:.1}",
        start.elapsed().as_secs_f32() * 1000.0,
        std::fs::metadata(&png_path)?.len()
    );

    // NFR-6: 1080p60 with <= 1% dropped frames and CPU <= 15%.
    let mp4_path = std::path::Path::new(out_dir).join("spike.mp4");
    println!("recording {seconds} s at {fps} fps...");
    let r = cb_platform::spike_record(&mp4_path, display.id, seconds, fps)?;
    println!("{r:#?}");
    println!(
        "dropped: {:.2}% | new screen frames: {:.1}/s | CPU: {:.1}% of {} threads | {:.1} Mbit/s",
        r.dropped_frames as f64 / r.target_frames as f64 * 100.0,
        r.fresh_frames as f64 / r.elapsed.as_secs_f64(),
        r.cpu_percent_of_machine,
        r.cores,
        r.file_bytes as f64 * 8.0 / r.elapsed.as_secs_f64() / 1e6,
    );
    Ok(())
}
