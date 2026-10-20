//! A streaming resampler for when the device does not run at the mixer's sample rate.
//!
//! The mixer always renders at its own rate, which never changes while it plays. When a
//! device runs at another rate, the mixer's frames are converted here with a 16-tap
//! windowed-sinc filter (Blackman window, 256 interpolated phases), the same design as
//! `audiobox`'s mixer uses for its sources. Every buffer is allocated when the resampler
//! is made, so [`Resampler::process`] allocates nothing.

/// Taps of the filter: input frames that each output frame weighs.
const TAPS: usize = 16;
/// Phases of the filter table, interpolated linearly between neighbours.
const PHASES: usize = 256;
/// Input frames kept before the current one, for the filter's left half.
const HALF: usize = TAPS / 2;
/// Input frames the resampler holds at most beyond its history.
const CAPACITY: usize = 2048;

pub(crate) struct Resampler {
    channels: usize,
    /// Input frames per output frame.
    step: f64,
    /// `PHASES + 1` rows of `TAPS` weights; the last row repeats the first, shifted.
    table: Box<[f32]>,
    /// Interleaved input frames: history first, then what the mixer rendered.
    input: Box<[f32]>,
    /// Valid frames in `input`.
    len: usize,
    /// The position of the next output frame, in input frames from the start of `input`.
    position: f64,
}

impl Resampler {
    /// A resampler from `from` to `to` frames a second, for `channels` interleaved
    /// channels.
    pub(crate) fn new(from: u32, to: u32, channels: usize) -> Self {
        let step = f64::from(from) / f64::from(to);
        // Below the lower Nyquist frequency, with a margin for the filter's slope.
        let cutoff = 0.9 * (f64::from(to) / f64::from(from)).min(1.0);
        let mut table = vec![0.0f32; (PHASES + 1) * TAPS];
        for phase in 0..=PHASES {
            let fraction = phase as f64 / PHASES as f64;
            let row = &mut table[phase * TAPS..(phase + 1) * TAPS];
            let mut sum = 0.0;
            let mut weights = [0.0f64; TAPS];
            for (tap, weight) in weights.iter_mut().enumerate() {
                // The tap's input frame is `HALF - 1 - tap` frames before the current
                // one, so its distance from the output's position is that plus the
                // fraction.
                let x = fraction + (HALF as f64 - 1.0 - tap as f64);
                *weight = cutoff * sinc(cutoff * x) * blackman(x);
                sum += *weight;
            }
            // Unit gain at DC for every phase.
            for (slot, weight) in row.iter_mut().zip(weights) {
                *slot = (weight / sum) as f32;
            }
        }
        Self {
            channels,
            step,
            table: table.into_boxed_slice(),
            input: vec![0.0; (CAPACITY + TAPS) * channels].into_boxed_slice(),
            // Silence before the first frame, as the filter's history.
            len: HALF - 1,
            position: (HALF - 1) as f64,
        }
    }

    /// Fills `out` with interleaved output frames, asking `pull` for input frames at the
    /// source's rate whenever it needs more. `pull` receives whole frames.
    pub(crate) fn process(&mut self, out: &mut [f32], mut pull: impl FnMut(&mut [f32])) {
        let channels = self.channels;
        let frames = out.len() / channels;
        for frame in 0..frames {
            let current = self.position as usize;
            if current + HALF >= self.len {
                // Keep the history that the next frames need, and render what follows.
                let keep_from = current + 1 - HALF;
                self.input
                    .copy_within(keep_from * channels..self.len * channels, 0);
                self.len -= keep_from;
                self.position -= keep_from as f64;
                let remaining = frames - frame;
                let wanted = ((remaining as f64 * self.step).ceil() as usize + TAPS).min(CAPACITY);
                let room = CAPACITY + TAPS - self.len;
                let count = wanted.min(room);
                pull(&mut self.input[self.len * channels..(self.len + count) * channels]);
                self.len += count;
            }
            let current = self.position as usize;
            let fraction = self.position - current as f64;
            let scaled = fraction * PHASES as f64;
            let phase = scaled as usize;
            let blend = (scaled - phase as f64) as f32;
            let lower = &self.table[phase * TAPS..(phase + 1) * TAPS];
            let upper = &self.table[(phase + 1) * TAPS..(phase + 2) * TAPS];
            let first = current + 1 - HALF;
            for channel in 0..channels {
                let mut sum = 0.0f32;
                for tap in 0..TAPS {
                    let weight = lower[tap] + (upper[tap] - lower[tap]) * blend;
                    // Tap 0 weighs the oldest frame of the window, HALF - 1 frames before
                    // the current one.
                    let index = first + tap;
                    sum += weight * self.input[index * channels + channel];
                }
                out[frame * channels + channel] = sum;
            }
            self.position += self.step;
        }
    }
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let y = std::f64::consts::PI * x;
        y.sin() / y
    }
}

/// A Blackman window over the filter's span, zero at its ends.
fn blackman(x: f64) -> f64 {
    let half = HALF as f64;
    if x.abs() >= half {
        return 0.0;
    }
    let t = std::f64::consts::PI * x / half;
    0.42 + 0.5 * t.cos() + 0.08 * (2.0 * t).cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sine at `frequency` and `rate`, from frame `start`.
    fn sine(frequency: f64, rate: u32, start: usize, out: &mut [f32]) {
        for (i, sample) in out.iter_mut().enumerate() {
            let t = (start + i) as f64 / f64::from(rate);
            *sample = (2.0 * std::f64::consts::PI * frequency * t).sin() as f32 * 0.5;
        }
    }

    /// Rising zero crossings a second, from the second half of `samples`.
    fn frequency(samples: &[f32], rate: u32) -> f64 {
        let half = &samples[samples.len() / 2..];
        let crossings: Vec<usize> = (1..half.len())
            .filter(|&i| half[i - 1] < 0.0 && half[i] >= 0.0)
            .collect();
        let periods = crossings.len() - 1;
        let span = crossings[crossings.len() - 1] - crossings[0];
        periods as f64 * f64::from(rate) / span as f64
    }

    #[test]
    fn a_sine_keeps_its_frequency_across_rates() {
        for (from, to) in [
            (44_100, 48_000),
            (48_000, 44_100),
            (48_000, 96_000),
            (32_000, 48_000),
        ] {
            let mut resampler = Resampler::new(from, to, 1);
            let mut produced = 0;
            let mut out = vec![0.0; to as usize];
            for chunk in out.chunks_mut(441) {
                resampler.process(chunk, |input| {
                    sine(1_000.0, from, produced, input);
                    produced += input.len();
                });
            }
            let measured = frequency(&out, to);
            assert!(
                (measured - 1_000.0).abs() < 0.5,
                "{from} to {to}: {measured} Hz"
            );
        }
    }

    #[test]
    fn a_constant_passes_at_unit_gain() {
        let mut resampler = Resampler::new(44_100, 48_000, 2);
        let mut out = vec![0.0; 2 * 4_800];
        resampler.process(&mut out, |input| input.fill(0.25));
        // After the filter's start-up, every sample is the constant.
        for sample in &out[2 * 64..] {
            assert!((sample - 0.25).abs() < 1e-4, "{sample}");
        }
    }

    #[test]
    fn output_length_follows_the_ratio() {
        let mut resampler = Resampler::new(48_000, 44_100, 1);
        let mut consumed = 0;
        let mut out = vec![0.0; 44_100];
        for chunk in out.chunks_mut(512) {
            resampler.process(chunk, |input| consumed += input.len());
        }
        // One second of output takes one second of input, within the filter's window and
        // the frames rendered ahead.
        assert!(
            (48_000..=48_000 + CAPACITY + TAPS).contains(&consumed),
            "{consumed}"
        );
    }
}
