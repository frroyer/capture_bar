//! Mixes audio sources onto the recording's timeline (US-5.3). No OS code: each backend
//! feeds it packets stamped with the time they were captured.
//!
//! Each source's device runs on its own clock, which drifts from the system clock the
//! video follows. A packet is placed right after the previous one of its source while
//! that stays within [`MAX_DRIFT`] of its own timestamp, and jumps to its timestamp
//! otherwise, so the sound never strays more than that from the picture.

/// Frames per second of every source and of the mix.
pub(crate) const RATE: u32 = 48_000;
/// Interleaved stereo.
pub(crate) const CHANNELS: usize = 2;
/// How far a source may slide from its timestamps before it is put back on them (US-5.3 AC: 40 ms).
pub(crate) const MAX_DRIFT: i64 = RATE as i64 / 50; // 20 ms
/// Placing a packet further ahead than this means a broken timestamp: it is dropped.
const MAX_AHEAD: i64 = RATE as i64 * 10;

pub(crate) struct Mixer {
    /// Frames handed out so far: `mix[0]` is the sample at this frame.
    written: i64,
    mix: Vec<f32>,
    /// Per source, the frame right after its last packet.
    next: Vec<Option<i64>>,
}

impl Mixer {
    pub fn new(sources: usize) -> Self {
        Self {
            written: 0,
            mix: Vec::new(),
            next: vec![None; sources],
        }
    }

    /// Add a packet of 16-bit stereo from `source`, captured at frame `at` of the timeline.
    pub fn add(&mut self, source: usize, at: i64, samples: &[i16]) {
        let frames = (samples.len() / CHANNELS) as i64;
        let start = match self.next[source] {
            Some(next) if (at - next).abs() <= MAX_DRIFT => next,
            _ => at,
        };
        self.next[source] = Some(start + frames);
        if start - self.written > MAX_AHEAD || start + frames <= self.written {
            return;
        }
        // Frames before `written` were already handed out: too late, dropped.
        let skip = (self.written - start).clamp(0, frames) as usize * CHANNELS;
        let offset = (start + skip as i64 / CHANNELS as i64 - self.written) as usize * CHANNELS;
        let samples = &samples[skip..];
        if self.mix.len() < offset + samples.len() {
            self.mix.resize(offset + samples.len(), 0.0);
        }
        for (mixed, &sample) in self.mix[offset..].iter_mut().zip(samples) {
            *mixed += f32::from(sample);
        }
    }

    /// The mix from the last call up to frame `until`, silent where no source played,
    /// with its first frame; `None` when there is nothing new.
    pub fn take(&mut self, until: i64) -> Option<(i64, Vec<i16>)> {
        if until <= self.written {
            return None;
        }
        let len = (until - self.written) as usize * CHANNELS;
        if self.mix.len() < len {
            self.mix.resize(len, 0.0);
        }
        let pcm = self
            .mix
            .drain(..len)
            .map(|s| s.clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16)
            .collect();
        let first = std::mem::replace(&mut self.written, until);
        Some((first, pcm))
    }

    /// The frame after the last one any source has given.
    pub fn end(&self) -> i64 {
        self.written + (self.mix.len() / CHANNELS) as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frames: usize, value: i16) -> Vec<i16> {
        vec![value; frames * CHANNELS]
    }

    #[test]
    fn small_jitter_keeps_packets_back_to_back() {
        let mut mixer = Mixer::new(1);
        mixer.add(0, 0, &tone(480, 100));
        // Stamped 5 ms late: still placed right after the first packet, no gap.
        mixer.add(0, 480 + 240, &tone(480, 100));
        let (first, pcm) = mixer.take(960).unwrap();
        assert_eq!(first, 0);
        assert!(pcm.iter().all(|&s| s == 100));
    }

    #[test]
    fn drift_beyond_the_limit_jumps_back_to_the_timestamp() {
        let mut mixer = Mixer::new(1);
        mixer.add(0, 0, &tone(480, 100));
        // A device clock 25 ms behind: the packet goes where its timestamp says.
        let late = 480 + MAX_DRIFT + 240;
        mixer.add(0, late, &tone(480, 100));
        let (_, pcm) = mixer.take(late + 480).unwrap();
        let at = |frame: i64| pcm[frame as usize * CHANNELS];
        assert_eq!(at(479), 100);
        assert_eq!(at(480), 0, "silence in the gap");
        assert_eq!(at(late), 100);
    }

    #[test]
    fn missing_packets_are_silence_and_late_ones_are_dropped() {
        let mut mixer = Mixer::new(1);
        let (first, pcm) = mixer.take(480).unwrap();
        assert_eq!((first, pcm.len()), (0, 480 * CHANNELS));
        assert!(pcm.iter().all(|&s| s == 0));
        // Wholly handed out already: dropped.
        mixer.add(0, 0, &tone(240, 9));
        // Half of it was already handed out.
        mixer.add(0, 240, &tone(480, 7));
        let (first, pcm) = mixer.take(720).unwrap();
        assert_eq!(first, 480);
        assert!(pcm.iter().all(|&s| s == 7));
        assert!(mixer.take(720).is_none());
    }

    #[test]
    fn sources_add_up_and_clip() {
        let mut mixer = Mixer::new(2);
        mixer.add(0, 0, &tone(100, 20_000));
        mixer.add(1, 50, &tone(100, 20_000));
        assert_eq!(mixer.end(), 150);
        let (_, pcm) = mixer.take(150).unwrap();
        let at = |frame: usize| pcm[frame * CHANNELS];
        assert_eq!(at(10), 20_000);
        assert_eq!(at(60), i16::MAX);
        assert_eq!(at(140), 20_000);
    }
}
