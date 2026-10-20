//! Plays [`audiobox`]'s real-time mixer on an audio device.
//!
//! `audiobox` makes and mixes the samples and stays free of dependencies and `unsafe`.
//! This crate carries them to the device: it opens the operating system's output stream
//! with [`cpal`], calls the mixer's `render` from the device's audio thread, and keeps the
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
//! - **Device changes.** [`AudioOutput::poll`] reopens the stream when the device
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
//! Through `cpal`: WASAPI on Windows, CoreAudio on macOS and iOS, ALSA (with PulseAudio
//! and PipeWire through it) on Linux, and AAudio on Android. The README says which have
//! run. The browser is a separate path, not yet in this crate.
mod backend;
mod cpal_backend;
mod error;
mod output;
mod render;
mod resample;

pub use backend::{
    Backend, BackendEvent, DeviceInfo, RenderCallback, StreamHandle, StreamPlan, StreamRequest,
};
pub use cpal_backend::CpalBackend;
pub use error::{Error, ErrorCode, Result};
pub use output::{AudioOutput, BUFFER_FRAMES, DeviceConfig, DeviceEvent, StreamInfo};

pub use audiobox;
pub use cpal;
