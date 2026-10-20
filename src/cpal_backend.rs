//! The backend for the operating system's audio, through `cpal`: WASAPI on Windows,
//! CoreAudio on macOS and iOS, ALSA (and PulseAudio or PipeWire through it) on Linux,
//! and AAudio on Android.
use std::sync::mpsc::SyncSender;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, SupportedBufferSize};

use crate::backend::{
    Backend, BackendEvent, DeviceInfo, RenderCallback, StreamHandle, StreamPlan, StreamRequest,
};
use crate::{Error, ErrorCode, Result};

/// Frames converted at a time for a device whose samples are not `f32`.
const SCRATCH_FRAMES: usize = 1024;

/// Sample formats in the order they are preferred.
const FORMATS: [SampleFormat; 8] = [
    SampleFormat::F32,
    SampleFormat::I16,
    SampleFormat::I32,
    SampleFormat::F64,
    SampleFormat::U16,
    SampleFormat::I8,
    SampleFormat::U8,
    SampleFormat::U32,
];

/// Plays through the operating system's audio with `cpal`.
pub struct CpalBackend {
    host: cpal::Host,
}

impl CpalBackend {
    /// The platform's default audio host.
    pub fn new() -> Self {
        Self {
            host: cpal::default_host(),
        }
    }

    /// Another of `cpal`'s hosts, such as JACK or ASIO where they are built in.
    pub fn with_host(host: cpal::Host) -> Self {
        Self { host }
    }

    fn default_device_id(&self) -> Option<String> {
        self.host
            .default_output_device()
            .and_then(|device| device.id().ok())
            .map(|id| id.to_string())
    }

    /// The device with this id, or the default device for `None`.
    fn device(&self, id: Option<&str>) -> Result<cpal::Device> {
        match id {
            None => self.host.default_output_device().ok_or_else(|| {
                Error::new(
                    ErrorCode::NoDevice,
                    "The system has no default output device.",
                )
            }),
            Some(id) => self
                .host
                .output_devices()
                .map_err(from_cpal)?
                .find(|device| device.id().is_ok_and(|found| found.to_string() == id))
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::DeviceNotFound,
                        format!("No output device has the id {id}."),
                    )
                }),
        }
    }
}

impl Default for CpalBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for CpalBackend {
    fn plan(&mut self, request: &StreamRequest) -> Result<StreamPlan> {
        let device = self.device(request.device.as_deref())?;
        let ranges: Vec<_> = device
            .supported_output_configs()
            .map_err(from_cpal)?
            .filter(|range| FORMATS.contains(&range.sample_format()))
            .collect();
        // The mixer's rate with enough channels, preferring as few channels as the mixer
        // needs and then the best sample format. Otherwise the device's default, through
        // the resampler.
        let rank = |format: SampleFormat| {
            FORMATS
                .iter()
                .position(|f| *f == format)
                .unwrap_or(FORMATS.len())
        };
        let chosen = ranges
            .iter()
            .filter(|range| {
                range.channels() >= request.channels && range.contains_rate(request.sample_rate)
            })
            .min_by_key(|range| {
                (
                    range.channels() - request.channels,
                    rank(range.sample_format()),
                )
            })
            .cloned()
            .map(|range| range.with_sample_rate(request.sample_rate));
        let config = match chosen {
            Some(config) => config,
            None => {
                let config = device.default_output_config().map_err(from_cpal)?;
                if !FORMATS.contains(&config.sample_format()) {
                    return Err(Error::new(
                        ErrorCode::UnsupportedConfig,
                        format!(
                            "The device's sample format {} is not supported.",
                            config.sample_format()
                        ),
                    ));
                }
                config
            }
        };
        let buffer_frames = match (request.buffer_frames, config.buffer_size()) {
            (None, _) => None,
            (Some(frames), SupportedBufferSize::Range { min, max }) => {
                Some(frames.clamp(*min, *max))
            }
            (Some(frames), SupportedBufferSize::Unknown) => Some(frames),
        };
        let description = device.description().map_err(from_cpal)?;
        Ok(StreamPlan {
            device_id: device.id().map_err(from_cpal)?.to_string(),
            device_name: description.name().to_owned(),
            is_default: request.device.is_none(),
            sample_rate: config.sample_rate(),
            channels: config.channels(),
            sample_format: config.sample_format().to_string(),
            buffer_frames,
        })
    }

    fn start(
        &mut self,
        plan: &StreamPlan,
        render: RenderCallback,
        events: SyncSender<BackendEvent>,
    ) -> Result<Box<dyn StreamHandle>> {
        // A stream built from the default device follows it on the backends that can.
        let device = self.device((!plan.is_default).then_some(plan.device_id.as_str()))?;
        let format = FORMATS
            .into_iter()
            .find(|format| format.to_string() == plan.sample_format)
            .ok_or_else(|| Error::new(ErrorCode::UnsupportedConfig, "Unknown sample format."))?;
        let config = cpal::StreamConfig {
            channels: plan.channels,
            sample_rate: plan.sample_rate,
            buffer_size: plan
                .buffer_frames
                .map_or(cpal::BufferSize::Default, cpal::BufferSize::Fixed),
        };
        let stream = match format {
            SampleFormat::F32 => build_f32(&device, &config, render, events)?,
            SampleFormat::I16 => build::<i16>(&device, &config, render, events)?,
            SampleFormat::I32 => build::<i32>(&device, &config, render, events)?,
            SampleFormat::F64 => build::<f64>(&device, &config, render, events)?,
            SampleFormat::U16 => build::<u16>(&device, &config, render, events)?,
            SampleFormat::I8 => build::<i8>(&device, &config, render, events)?,
            SampleFormat::U8 => build::<u8>(&device, &config, render, events)?,
            SampleFormat::U32 => build::<u32>(&device, &config, render, events)?,
            _ => {
                return Err(Error::new(
                    ErrorCode::UnsupportedConfig,
                    "Unsupported sample format.",
                ));
            }
        };
        stream.play().map_err(from_cpal)?;
        Ok(Box::new(CpalStream(stream)))
    }

    fn default_device(&mut self) -> Option<String> {
        self.default_device_id()
    }

    fn devices(&mut self) -> Result<Vec<DeviceInfo>> {
        let default = self.default_device_id();
        let mut devices = Vec::new();
        for device in self.host.output_devices().map_err(from_cpal)? {
            let Ok(id) = device.id() else { continue };
            let id = id.to_string();
            let name = device
                .description()
                .map(|description| description.name().to_owned())
                .unwrap_or_else(|_| id.clone());
            devices.push(DeviceInfo {
                is_default: default.as_deref() == Some(id.as_str()),
                id,
                name,
            });
        }
        Ok(devices)
    }
}

struct CpalStream(cpal::Stream);

impl StreamHandle for CpalStream {
    fn buffer_frames(&self) -> Option<u32> {
        self.0.buffer_size().ok()
    }
}

/// A stream whose samples are `f32`, rendered in place.
fn build_f32(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut render: RenderCallback,
    events: SyncSender<BackendEvent>,
) -> Result<cpal::Stream> {
    device
        .build_output_stream::<f32, _, _>(
            *config,
            move |data, _| render(data),
            move |error| report(&events, error),
            None,
        )
        .map_err(from_cpal)
}

/// A stream of another sample format, rendered in chunks into a buffer allocated here
/// and converted.
fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut render: RenderCallback,
    events: SyncSender<BackendEvent>,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let channels = usize::from(config.channels.max(1));
    let mut scratch = vec![0.0f32; SCRATCH_FRAMES * channels].into_boxed_slice();
    device
        .build_output_stream::<T, _, _>(
            *config,
            move |data, _| {
                for chunk in data.chunks_mut(SCRATCH_FRAMES * channels) {
                    let samples = &mut scratch[..chunk.len()];
                    render(samples);
                    for (target, sample) in chunk.iter_mut().zip(samples.iter()) {
                        *target = T::from_sample(*sample);
                    }
                }
            },
            move |error| report(&events, error),
            None,
        )
        .map_err(from_cpal)
}

/// Sends what a stream error means to the output, without blocking.
fn report(events: &SyncSender<BackendEvent>, error: cpal::Error) {
    let event = match error.kind() {
        cpal::ErrorKind::DeviceChanged => BackendEvent::Rerouted,
        cpal::ErrorKind::DeviceNotAvailable | cpal::ErrorKind::StreamInvalidated => {
            BackendEvent::Lost(from_cpal(error))
        }
        cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied => {
            BackendEvent::Warning(from_cpal(error))
        }
        _ => BackendEvent::Lost(from_cpal(error)),
    };
    let _ = events.try_send(event);
}

fn from_cpal(error: cpal::Error) -> Error {
    let code = match error.kind() {
        cpal::ErrorKind::DeviceNotAvailable => ErrorCode::DeviceUnavailable,
        cpal::ErrorKind::DeviceBusy => ErrorCode::DeviceBusy,
        cpal::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
        cpal::ErrorKind::HostUnavailable => ErrorCode::HostUnavailable,
        cpal::ErrorKind::UnsupportedConfig | cpal::ErrorKind::UnsupportedOperation => {
            ErrorCode::UnsupportedConfig
        }
        cpal::ErrorKind::InvalidInput => ErrorCode::InvalidArgument,
        _ => ErrorCode::StreamFailed,
    };
    Error::new(code, error.to_string())
}
