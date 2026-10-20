//! The seam between [`AudioOutput`](crate::AudioOutput) and an audio system.
//!
//! [`CpalBackend`](crate::CpalBackend) is the real one. Tests, and hosts with an audio
//! system of their own, implement [`Backend`] to drive the same output logic.
use std::sync::mpsc::SyncSender;

use crate::Result;

/// The render callback that a backend calls on its audio thread. It fills an
/// interleaved buffer of the stream's channels with `f32` samples in -1..=1, and
/// allocates nothing, takes no lock that could wait, and does not block.
pub type RenderCallback = Box<dyn FnMut(&mut [f32]) + Send + 'static>;

/// What [`AudioOutput`](crate::AudioOutput) asks of a backend when it opens a stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamRequest {
    /// The device's id, or `None` for the default output device.
    pub device: Option<String>,
    /// The mixer's sample rate, which the backend uses if the device supports it.
    pub sample_rate: u32,
    /// The mixer's channels, 1 or 2. The backend prefers a device configuration with at
    /// least as many.
    pub channels: u16,
    /// Frames per callback, or `None` for the device's default.
    pub buffer_frames: Option<u32>,
}

/// The configuration that a backend chose for a stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamPlan {
    /// The device's stable id, which [`StreamRequest::device`] accepts.
    pub device_id: String,
    /// The device's human-readable name.
    pub device_name: String,
    /// Whether the stream plays on the system's default device and follows it.
    pub is_default: bool,
    pub sample_rate: u32,
    pub channels: u16,
    /// The device's sample format, such as `f32` or `i16`. The backend converts to it.
    pub sample_format: String,
    /// The frames per callback that were requested, if a fixed size was.
    pub buffer_frames: Option<u32>,
}

/// Something that a backend reports from its audio system, through the sink that
/// [`Backend::start`] receives.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BackendEvent {
    /// The system moved the stream to another device, such as new headphones, and it
    /// keeps playing.
    Rerouted,
    /// The device disappeared or the stream stopped working: it has to be reopened.
    Lost(crate::Error),
    /// A problem that did not stop the stream, such as an underrun.
    Warning(crate::Error),
}

/// A playing stream. Dropping it stops the stream.
pub trait StreamHandle: Send {
    /// Frames per callback as the device reports them, if it does.
    fn buffer_frames(&self) -> Option<u32> {
        None
    }
}

/// An audio system that can open output streams.
pub trait Backend: Send {
    /// Chooses a configuration for `request`, without starting anything.
    fn plan(&mut self, request: &StreamRequest) -> Result<StreamPlan>;
    /// Starts a stream with `plan`, which [`Backend::plan`] returned. The backend calls
    /// `render` for every buffer, and sends what happens to the stream to `events`
    /// without blocking: with `try_send`, dropping an event when the queue is full.
    fn start(
        &mut self,
        plan: &StreamPlan,
        render: RenderCallback,
        events: SyncSender<BackendEvent>,
    ) -> Result<Box<dyn StreamHandle>>;
    /// The id of the system's current default output device, if there is one.
    fn default_device(&mut self) -> Option<String>;
    /// The output devices, as ids and names.
    fn devices(&mut self) -> Result<Vec<DeviceInfo>>;
}

/// An output device.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceInfo {
    /// The device's stable id, which [`DeviceConfig::device`](crate::DeviceConfig) accepts.
    pub id: String,
    /// The device's human-readable name.
    pub name: String,
    /// Whether it is the system's default output device.
    pub is_default: bool,
}
