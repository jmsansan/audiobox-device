//! What runs on the audio thread: the mixer's frames, converted to the device's rate and
//! channels.
use std::sync::{Arc, Mutex};

use audiobox::mixer::Mixer;

use crate::resample::Resampler;

/// Frames rendered at a time, the size of the buffer allocated for them up front.
const CHUNK: usize = 1024;

pub(crate) struct Renderer {
    mixer: Arc<Mutex<Mixer>>,
    mixer_channels: usize,
    device_channels: usize,
    /// The mixer's frames, before they are mapped onto the device's channels.
    mix: Box<[f32]>,
    /// Present when the device runs at another rate than the mixer.
    resampler: Option<Resampler>,
}

impl Renderer {
    pub(crate) fn new(
        mixer: Arc<Mutex<Mixer>>,
        mixer_rate: u32,
        mixer_channels: usize,
        device_rate: u32,
        device_channels: usize,
    ) -> Self {
        Self {
            mixer,
            mixer_channels,
            device_channels: device_channels.max(1),
            mix: vec![0.0; CHUNK * mixer_channels].into_boxed_slice(),
            resampler: (mixer_rate != device_rate)
                .then(|| Resampler::new(mixer_rate, device_rate, mixer_channels)),
        }
    }

    /// Fills `out`, interleaved at the device's channels, from the mixer. Allocates
    /// nothing and never waits: if the mixer is held elsewhere, which happens only for an
    /// instant while a stream is replaced, the buffer is silent.
    pub(crate) fn render(&mut self, out: &mut [f32]) {
        let device_channels = self.device_channels;
        let mixer_channels = self.mixer_channels;
        let frames = out.len() / device_channels;
        let Ok(mut mixer) = self.mixer.try_lock() else {
            out.fill(0.0);
            return;
        };
        let mut done = 0;
        while done < frames {
            let count = (frames - done).min(CHUNK);
            let mix = &mut self.mix[..count * mixer_channels];
            match &mut self.resampler {
                None => mixer.render(mix),
                Some(resampler) => resampler.process(mix, |input| mixer.render(input)),
            }
            map(
                mix,
                &mut out[done * device_channels..(done + count) * device_channels],
                mixer_channels,
                device_channels,
            );
            done += count;
        }
        out[frames * device_channels..].fill(0.0);
    }
}

/// Copies frames of `from` channels onto `to` channels, clamped to -1..=1.
/// - The same count: as they are.
/// - Mono onto two or more: the left and right channels, and silence on the rest.
/// - Stereo onto mono: the average.
/// - Stereo onto more: the first two, and silence on the rest.
pub(crate) fn map(input: &[f32], output: &mut [f32], from: usize, to: usize) {
    let clamp = |sample: f32| sample.clamp(-1.0, 1.0);
    for (source, target) in input.chunks_exact(from).zip(output.chunks_exact_mut(to)) {
        match (from, to) {
            (from, to) if from == to => {
                for (s, t) in source.iter().zip(target.iter_mut()) {
                    *t = clamp(*s);
                }
            }
            (1, _) => {
                let sample = clamp(source[0]);
                target[0] = sample;
                target[1] = sample;
                target[2..].fill(0.0);
            }
            (_, 1) => target[0] = clamp((source[0] + source[1]) * 0.5),
            _ => {
                target[0] = clamp(source[0]);
                target[1] = clamp(source[1]);
                target[2..].fill(0.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::map;

    #[test]
    fn channels_map_as_documented() {
        let mut out = [9.0; 4];
        map(&[0.5, -0.5], &mut out, 1, 2);
        assert_eq!(out, [0.5, 0.5, -0.5, -0.5]);

        let mut out = [9.0; 1];
        map(&[0.25, 0.75], &mut out, 2, 1);
        assert_eq!(out, [0.5]);

        let mut out = [9.0; 6];
        map(&[0.1, 0.2], &mut out, 2, 6);
        assert_eq!(out, [0.1, 0.2, 0.0, 0.0, 0.0, 0.0]);

        let mut out = [9.0; 2];
        map(&[2.0, -3.0], &mut out, 2, 2);
        assert_eq!(out, [1.0, -1.0]);
    }
}
