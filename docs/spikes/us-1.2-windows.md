# US-1.2 spike: Windows.Graphics.Capture + Media Foundation

Capture and H.264 encoding work on Windows, and the grab meets NFR-4. The CPU copy path misses
NFR-6's dropped-frame target at 4–5%, so US-5.1 should test zero-copy GPU frames next.

Measured 2026-11-06 with a release build:

```
cargo run -p cb-platform --example spike --release -- <out-dir> [display-index] [seconds] [fps]
```

The output files contain whatever is on screen. Keep them out of the repo.

**Machine:** Intel Core Ultra 5 235H (14 logical CPUs), Intel Arc Pro 140T, Windows 11 Enterprise.
This is not the "4-core 2020 laptop" NFR-6 names, so its CPU figure is only indicative.

## Results

10 s at 60 fps, 600 frame slots. An 800×600 window of smooth motion was on the recorded screen.

| Metric | Target | Laptop panel, 1920×1200 | DisplayLink, 1920×1080 | Verdict |
| --- | --- | --- | --- | --- |
| Grab, median of 20 (min–max) | ≤ 100 ms (NFR-4) | 52.0 ms (44.3–73.1) | 50.7 ms (47.7–71.4) | Met for the grab |
| Dropped frame slots | ≤ 1% (NFR-6) | 5.00% | 4.17% | **Missed** |
| New screen frames per second | 60 | 36.2 | 46.0 | **Open** |
| Process CPU, share of 14 threads | ≤ 15% (NFR-6) | 3.8% | 4.0% | Not proven |
| Encoder | Hardware first | Hardware | Hardware | Met |
| Bitrate | none set | 12.5 Mbit/s | 11.2 Mbit/s | Set to 0.1 bit/pixel |

- **NFR-4 scope:** the grab excludes the PNG worker (11–23 ms in runs before the final code) and
  buffer append. Only the first grab after launch pays for the device and capture item.
- **CPU:** 4% of 14 threads is about 0.55 of a core, roughly 14% of a 4-core machine if per-core
  speed were equal. GPU encoder load is not counted.
- **Encoder:** the transform reports itself as asynchronous with a hardware URL but gives no name.
  Intel Quick Sync is the hardware H.264 encoder this machine lists.
- **MP4 validity:** each file decodes through Media Foundation. The decoder returned 600 frames
  even when only 570–575 were written, so the decoded count does not confirm the written count.

## Decisions

- **Device and capture item are cached per process.** Without caching a grab took 200 ms: device
  creation about 75 ms, capture item about 77 ms. No session stays open while idle (NFR-7).
- **The sink writer needs a D3D device manager to use the hardware encoder.** Without one it picked
  the software encoder, at 8.5% CPU in the only run taken that way (not a motion test).

## Open

1. **Dropped frames and new-frame rate.** The rates are low on both screens, so DisplayLink is not
   the sole cause. The likely cause is the CPU path (copy to staging, map, convert, upload), but it
   has not been profiled. Test GPU textures to the encoder in US-5.1.
2. **Run variance.** Earlier runs with a different test window on DisplayLink dropped 0.17–0.67%,
   with 18 new frames per second. Motion content and window size change the results.
3. **Not exercised:** the 4-core reference laptop, Windows 10 1903, ARM64, Window capture (US-4.3).
4. ~~**`start_stream` is still a stub.**~~ Done in US-5.1, see below.

## US-5.1 follow-up: GPU path

Measured 2026-11-08, same machine, release build, primary display 1920×1200 at 60 fps:

```
cargo test -p cb-platform --release --test record -- --ignored --nocapture
```

| Metric | Target | CPU path (spike) | GPU path (US-5.1) |
| --- | --- | --- | --- |
| Dropped frame slots, 10 s | ≤ 1% (NFR-6) | 5.00% | 0 of 587, three runs |
| Start, request to first frame | ≤ 300 ms (US-5.1) | not measured | 237–249 ms after the launch warm-up; ~650 ms without it |

- **What changed:** each new capture frame is cropped into its own D3D11 texture and given to the
  sink writer as a DXGI surface buffer, so nothing is mapped or copied on the CPU.
- **Input format:** the sink writer only accepts `B8G8R8A8` textures declared as `MFVideoFormat_ARGB32`.
  `RGB32` fails at `Finalize` with `E_INVALIDARG`.
- **Drops:** a slot's time in the file comes from its index, so a slot written a little late plays on time.
  Only lagging by more than 4 slots skips any. With a 1-slot limit, scheduler jitter alone dropped 0–1.2%.
- **Start latency:** a warm process spends ~15 ms on the capture session and ~215 ms opening the sink
  writer. The first recording in a process also loads the GPU device, the capture item and the hardware
  encoder. The app pays that on a thread at launch.
- **Not measured yet:** motion on screen during the 10 s run, CPU share, the 4-core reference laptop.
