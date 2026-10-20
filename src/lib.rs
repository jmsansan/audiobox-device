//! Plays [`audiobox`]'s real-time mixer on an audio device.
//!
//! `audiobox` makes and mixes the samples and stays free of dependencies and `unsafe`.
//! This crate carries them to the device: it opens the operating system's output stream
//! with `cpal`, calls the mixer's `render` from the device's audio thread, and keeps the
//! sound playing when the device changes.
//!
//! ```no_run
//! use audiobox::mixer::{Mixer, MixerConfig, Play};
//! use audiobox_device::{AudioOutput, DeviceConfig};
//!
//! let (mixer, mut handle) = Mixer::new(MixerConfig::default())?;
//! let mut output = AudioOutput::open(mixer, &DeviceConfig::default())?;
//! println!("Playing on {}", output.info().unwrap().device_name);
//!
//! // Start voices with the handle, and poll the output once per frame.
//! # let sound = std::sync::Arc::new(audiobox::Audio::new(vec![vec![0.0; 480]], 48_000)?);
//! handle.play(sound, &Play::default())?;
//! while let Some(event) = output.poll() {
//!     println!("{event:?}");
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # What the output does
//!
//! - **Configuration.** It opens the default device, or a named one, preferring the
//!   mixer's sample rate and at least its channels, and `f32` samples, and converts to
//!   what the device takes.
//! - **Rates.** A device that does not run at the mixer's rate gets the mixer's frames
//!   through a 16-tap windowed-sinc resampler. The mixer's rate never changes, so its
//!   voices, fades and scheduled changes keep their timing.
//! - **Channels.** A stereo mixer plays on the first two channels of a surround device,
//!   and is averaged on a mono one.
//! - **Device changes.** `AudioOutput::poll` reopens the stream when the device
//!   disappears, its configuration changes, or the system's default device changes, and
//!   the mixer keeps its voices.
//!
//! # Real-time safety
//!
//! The device's callback renders into buffers allocated when the stream is opened. It
//! allocates nothing and never waits: the mixer sits behind a mutex that only the
//! callback locks, with `try_lock`, so a buffer that could not take it, which can happen
//! only while a stream is being replaced, is silent rather than late.
//!
//! # Platforms
//!
//! Natively through `cpal`: WASAPI on Windows, CoreAudio on macOS and iOS, ALSA (with
//! PulseAudio and PipeWire through it) on Linux, and AAudio on Android. In the browser,
//! with the `web` feature (on by default) on `wasm32-unknown-unknown`, through an
//! `AudioWorklet`: see `WebOutput`. The README says which have run.
//!
//! # One API for both
//!
//! A [`Player`] loads sounds and plays them by id, the same way natively and in the
//! browser. Natively, `AudioOutput::open_player` returns one; in the browser,
//! `WebOutput::open` does. Sounds are loaded once because, in the browser, the mixer runs
//! in a WebAssembly instance of its own on the audio thread; a score of `audiobox`'s
//! synthesizer travels there as its text and is synthesized as it plays.
#[cfg(not(target_arch = "wasm32"))]
mod backend;
#[cfg(not(target_arch = "wasm32"))]
mod cpal_backend;
mod error;
#[cfg(not(target_arch = "wasm32"))]
mod output;
mod player;
#[cfg_attr(
    not(all(target_arch = "wasm32", feature = "web")),
    allow(
        dead_code,
        reason = "the page's half of the protocol runs in the browser; tests use it natively"
    )
)]
mod protocol;
#[cfg(not(target_arch = "wasm32"))]
mod render;
#[cfg(not(target_arch = "wasm32"))]
mod resample;
#[cfg(all(target_arch = "wasm32", feature = "web"))]
mod web;
mod worklet;

#[cfg(not(target_arch = "wasm32"))]
pub use backend::{
    Backend, BackendEvent, DeviceInfo, RenderCallback, StreamHandle, StreamPlan, StreamRequest,
};
#[cfg(not(target_arch = "wasm32"))]
pub use cpal_backend::CpalBackend;
pub use error::{Error, ErrorCode, Result};
#[cfg(not(target_arch = "wasm32"))]
pub use output::{AudioOutput, BUFFER_FRAMES, DeviceConfig, DeviceEvent, StreamInfo};
pub use player::{Bus, PlayOptions, Player, PlayerEvent, Scheduled, SoundId, VoiceId};
#[cfg(all(target_arch = "wasm32", feature = "web"))]
pub use web::WebOutput;

/// The engine that the crate `audiobox-device-worklet` wraps for the browser's audio
/// worklet. Not part of the stable API.
#[doc(hidden)]
pub mod __worklet {
    pub use crate::worklet::Engine;
}

pub use audiobox;
#[cfg(not(target_arch = "wasm32"))]
pub use cpal;

#[cfg(test)]
mod tests;
