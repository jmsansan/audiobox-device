use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use audiobox::mixer::Mixer;

use crate::backend::{Backend, BackendEvent, DeviceInfo, StreamHandle, StreamPlan, StreamRequest};
use crate::render::Renderer;
use crate::{CpalBackend, Error, ErrorCode, Result};

/// Smallest and largest buffer that [`DeviceConfig::buffer_frames`] accepts.
pub const BUFFER_FRAMES: std::ops::RangeInclusive<u32> = 16..=16_384;
/// How often [`AudioOutput::poll`] checks whether the system's default device changed,
/// and retries a stream that could not be reopened.
const CHECK_EVERY: Duration = Duration::from_secs(1);
/// Backend events that can wait for [`AudioOutput::poll`].
const EVENT_QUEUE: usize = 64;

/// Where and how to play.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeviceConfig {
    /// A device's id from [`AudioOutput::devices`], or `None` for the system's default
    /// output device, which the output follows when the system changes it.
    pub device: Option<String>,
    /// Frames per callback, within [`BUFFER_FRAMES`], or `None` for the device's default.
    /// Smaller buffers lower the latency and raise the risk of dropouts; 256 to 1024
    /// frames at 48 kHz (5 to 21 ms) suit most games.
    pub buffer_frames: Option<u32>,
}

/// The stream that is playing.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamInfo {
    pub device_id: String,
    pub device_name: String,
    /// Whether the output plays on the system's default device and follows it.
    pub is_default: bool,
    /// The device's sample rate.
    pub sample_rate: u32,
    /// The device's channels.
    pub channels: u16,
    /// The device's sample format, such as `f32` or `i16`.
    pub sample_format: String,
    /// Frames per callback, as requested or as the device reports them.
    pub buffer_frames: Option<u32>,
    /// The latency that one buffer adds, when its size is known. The device adds its own.
    pub buffer_latency: Option<Duration>,
    /// Whether the mixer's frames are resampled, because the device does not run at the
    /// mixer's rate.
    pub resampled: bool,
}

/// What happened to the output, reported by [`AudioOutput::poll`].
#[derive(Clone, Debug, PartialEq)]
pub enum DeviceEvent {
    /// The stream was opened again, on this device and configuration, and the mixer
    /// kept its voices: after the device disappeared, its configuration changed, or the
    /// system's default device changed.
    Reopened(StreamInfo),
    /// The system moved the stream to another device on its own, and it keeps playing.
    Rerouted,
    /// The device chosen in [`DeviceConfig::device`] is gone, so the output plays on the
    /// default device.
    FellBackToDefault(Error),
    /// The stream could not be opened again. The output is silent and retries on later
    /// polls.
    Failed(Error),
    /// A problem that did not stop the stream, such as an underrun.
    Warning(Error),
}

/// Plays a [`Mixer`] on an audio device.
///
/// The output owns the mixer and calls its `render` from the device's audio thread; the
/// game keeps the [`MixerHandle`](audiobox::mixer::MixerHandle). Call [`poll`] regularly,
/// such as once per frame: it reopens the stream when the device disappears or the
/// default device changes, and reports what happened. The mixer keeps its voices across
/// a reopening, and plays at its own sample rate whatever the device's, through a
/// resampler when they differ.
///
/// [`poll`]: AudioOutput::poll
pub struct AudioOutput<B: Backend = CpalBackend> {
    backend: B,
    config: DeviceConfig,
    mixer: Arc<Mutex<Mixer>>,
    mixer_rate: u32,
    mixer_channels: usize,
    stream: Option<Box<dyn StreamHandle>>,
    info: Option<StreamInfo>,
    events_in: SyncSender<BackendEvent>,
    events: Receiver<BackendEvent>,
    pending: VecDeque<DeviceEvent>,
    /// Whether the stream must be opened again at the next poll.
    lost: bool,
    last_check: Instant,
}

impl AudioOutput<CpalBackend> {
    /// Plays `mixer` on the operating system's audio device.
    pub fn open(mixer: Mixer, config: &DeviceConfig) -> Result<Self> {
        Self::with_backend(CpalBackend::new(), mixer, config)
    }

    /// The operating system's output devices.
    pub fn devices() -> Result<Vec<DeviceInfo>> {
        CpalBackend::new().devices()
    }
}

impl<B: Backend> AudioOutput<B> {
    /// Plays `mixer` through `backend`.
    pub fn with_backend(backend: B, mixer: Mixer, config: &DeviceConfig) -> Result<Self> {
        if let Some(frames) = config.buffer_frames
            && !BUFFER_FRAMES.contains(&frames)
        {
            return Err(Error::new(
                ErrorCode::InvalidArgument,
                format!(
                    "A buffer of {frames} frames is outside {} to {}.",
                    BUFFER_FRAMES.start(),
                    BUFFER_FRAMES.end()
                ),
            ));
        }
        let (events_in, events) = sync_channel(EVENT_QUEUE);
        let mut output = Self {
            backend,
            config: config.clone(),
            mixer_rate: mixer.sample_rate(),
            mixer_channels: mixer.channels(),
            mixer: Arc::new(Mutex::new(mixer)),
            stream: None,
            info: None,
            events_in,
            events,
            pending: VecDeque::new(),
            lost: false,
            last_check: Instant::now(),
        };
        let info = output.start(output.config.device.clone())?;
        output.info = Some(info);
        Ok(output)
    }

    /// The stream that is playing, or `None` while it could not be reopened.
    pub fn info(&self) -> Option<&StreamInfo> {
        self.info.as_ref()
    }

    /// The backend, for hosts and tests that drive it.
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Handles what happened to the stream since the last call, and returns the next
    /// event, or `None`. Call it until it returns `None`, regularly, such as once per
    /// frame. It never blocks; reopening a stream takes as long as the system takes.
    pub fn poll(&mut self) -> Option<DeviceEvent> {
        loop {
            match self.events.try_recv() {
                Ok(BackendEvent::Rerouted) => self.pending.push_back(DeviceEvent::Rerouted),
                Ok(BackendEvent::Warning(error)) => {
                    self.pending.push_back(DeviceEvent::Warning(error))
                }
                Ok(BackendEvent::Lost(_)) => self.lost = true,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        let now = Instant::now();
        let due = now.duration_since(self.last_check) >= CHECK_EVERY;
        if due {
            self.last_check = now;
            // A default device that changed without the stream following it.
            if let Some(info) = &self.info
                && info.is_default
                && self
                    .backend
                    .default_device()
                    .is_some_and(|id| id != info.device_id)
            {
                self.lost = true;
            }
        }
        // Reopen at once after a loss; after a failed attempt, retry once a second.
        if self.lost && (self.info.is_some() || due) {
            self.reopen_now();
        }
        self.pending.pop_front()
    }

    /// Opens the stream again at once, on the configured device, or on the default
    /// device if that one is gone.
    pub fn reopen(&mut self) -> Result<&StreamInfo> {
        self.lost = true;
        self.reopen_now();
        match &self.info {
            Some(info) => Ok(info),
            None => Err(Error::new(
                ErrorCode::StreamFailed,
                "The stream could not be opened again.",
            )),
        }
    }

    fn reopen_now(&mut self) {
        // The old stream's callback holds a clone of the mixer; dropping it first lets
        // the new stream's callback take the mixer without waiting.
        self.stream = None;
        self.info = None;
        // Events of the old stream no longer apply.
        while self.events.try_recv().is_ok() {}
        let wanted = self.config.device.clone();
        let result = match self.start(wanted.clone()) {
            Err(error)
                if wanted.is_some()
                    && matches!(
                        error.code,
                        ErrorCode::DeviceNotFound | ErrorCode::DeviceUnavailable
                    ) =>
            {
                self.pending
                    .push_back(DeviceEvent::FellBackToDefault(error));
                self.start(None)
            }
            other => other,
        };
        match result {
            Ok(info) => {
                self.lost = false;
                self.info = Some(info.clone());
                self.pending.push_back(DeviceEvent::Reopened(info));
            }
            Err(error) => self.pending.push_back(DeviceEvent::Failed(error)),
        }
    }

    fn start(&mut self, device: Option<String>) -> Result<StreamInfo> {
        let request = StreamRequest {
            device,
            sample_rate: self.mixer_rate,
            channels: self.mixer_channels as u16,
            buffer_frames: self.config.buffer_frames,
        };
        let plan = self.backend.plan(&request)?;
        let mut renderer = Renderer::new(
            Arc::clone(&self.mixer),
            self.mixer_rate,
            self.mixer_channels,
            plan.sample_rate,
            usize::from(plan.channels),
        );
        let stream = self.backend.start(
            &plan,
            Box::new(move |out| renderer.render(out)),
            self.events_in.clone(),
        )?;
        let info = self.describe(&plan, stream.as_ref());
        self.stream = Some(stream);
        Ok(info)
    }

    fn describe(&self, plan: &StreamPlan, stream: &dyn StreamHandle) -> StreamInfo {
        let buffer_frames = stream.buffer_frames().or(plan.buffer_frames);
        StreamInfo {
            device_id: plan.device_id.clone(),
            device_name: plan.device_name.clone(),
            is_default: plan.is_default,
            sample_rate: plan.sample_rate,
            channels: plan.channels,
            sample_format: plan.sample_format.clone(),
            buffer_frames,
            buffer_latency: buffer_frames.map(|frames| {
                Duration::from_secs_f64(f64::from(frames) / f64::from(plan.sample_rate))
            }),
            resampled: plan.sample_rate != self.mixer_rate,
        }
    }
}
