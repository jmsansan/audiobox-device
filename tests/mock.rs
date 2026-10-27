//! The output's logic through a simulated device: what it asks of the mixer, how it
//! reopens, and that its callback allocates nothing.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use audiobox::Audio;
use audiobox::mixer::{Loop, Mixer, MixerConfig, MixerHandle, Play};
use audiobox_device::{
    AudioOutput, Backend, BackendEvent, DeviceConfig, DeviceEvent, DeviceInfo, Error, ErrorCode,
    RenderCallback, Result, StreamHandle, StreamPlan, StreamRequest,
};

/// Counts the allocations of the current thread while counting is on.
struct Counting;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.with(Cell::get) {
            ALLOCATIONS.with(|count| count.set(count.get() + 1));
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// A simulated audio system: one default device and one named device, whose rate and
/// channels the test sets. The test runs the stream's callback itself.
#[derive(Clone, Default)]
struct Mock(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    rate: u32,
    channels: u16,
    default_id: String,
    named_present: bool,
    callback: Option<RenderCallback>,
    events: Option<SyncSender<BackendEvent>>,
    starts: usize,
}

impl Mock {
    fn new(rate: u32, channels: u16) -> Self {
        Self(Arc::new(Mutex::new(State {
            rate,
            channels,
            default_id: "speakers".into(),
            named_present: true,
            ..State::default()
        })))
    }

    /// Runs the stream's callback for `frames` frames and returns what it wrote.
    fn pull(&self, frames: usize) -> Vec<f32> {
        let mut state = self.0.lock().unwrap();
        let channels = usize::from(state.channels);
        let mut buffer = vec![0.0; frames * channels];
        (state.callback.as_mut().expect("a stream plays"))(&mut buffer);
        buffer
    }

    /// Runs the callback with counting on, and returns the allocations it made.
    fn allocations_in_pull(&self, frames: usize) -> usize {
        let mut state = self.0.lock().unwrap();
        let channels = usize::from(state.channels);
        let mut buffer = vec![0.0; frames * channels];
        let callback = state.callback.as_mut().expect("a stream plays");
        ALLOCATIONS.with(|count| count.set(0));
        COUNTING.with(|on| on.set(true));
        callback(&mut buffer);
        COUNTING.with(|on| on.set(false));
        ALLOCATIONS.with(Cell::get)
    }

    fn report(&self, event: BackendEvent) {
        let state = self.0.lock().unwrap();
        state.events.as_ref().unwrap().try_send(event).unwrap();
    }

    fn set_rate(&self, rate: u32) {
        self.0.lock().unwrap().rate = rate;
    }

    fn starts(&self) -> usize {
        self.0.lock().unwrap().starts
    }
}

struct MockStream;
impl StreamHandle for MockStream {
    fn buffer_frames(&self) -> Option<u32> {
        Some(512)
    }
}

impl Backend for Mock {
    fn plan(&mut self, request: &StreamRequest) -> Result<StreamPlan> {
        let state = self.0.lock().unwrap();
        let (id, name) = match request.device.as_deref() {
            None => (state.default_id.clone(), "Default speakers".to_owned()),
            Some("usb") if state.named_present => ("usb".to_owned(), "USB headset".to_owned()),
            Some(other) => {
                return Err(Error::new(
                    ErrorCode::DeviceNotFound,
                    format!("No device {other}."),
                ));
            }
        };
        Ok(StreamPlan {
            device_id: id,
            device_name: name,
            is_default: request.device.is_none(),
            sample_rate: state.rate,
            channels: state.channels,
            sample_format: "f32".into(),
            buffer_frames: request.buffer_frames,
        })
    }

    fn start(
        &mut self,
        _plan: &StreamPlan,
        render: RenderCallback,
        events: SyncSender<BackendEvent>,
    ) -> Result<Box<dyn StreamHandle>> {
        let mut state = self.0.lock().unwrap();
        state.callback = Some(render);
        state.events = Some(events);
        state.starts += 1;
        Ok(Box::new(MockStream))
    }

    fn default_device(&mut self) -> Option<String> {
        Some(self.0.lock().unwrap().default_id.clone())
    }

    fn devices(&mut self) -> Result<Vec<DeviceInfo>> {
        Ok(Vec::new())
    }
}

/// A 48 kHz stereo mixer playing a looping mono constant of 0.25 in the centre.
fn mixer_with_tone() -> (Mixer, MixerHandle) {
    let (mixer, mut handle) = Mixer::new(MixerConfig::default()).unwrap();
    let tone = Arc::new(Audio::new(vec![vec![0.25; 4_800]], 48_000).unwrap());
    handle
        .play(
            tone,
            &Play {
                looping: Loop::Whole,
                ..Play::default()
            },
        )
        .unwrap();
    (mixer, handle)
}

fn drain(output: &mut AudioOutput<Mock>) -> Vec<DeviceEvent> {
    std::iter::from_fn(|| output.poll()).collect()
}

#[test]
fn the_callback_asks_the_mixer_for_the_frames_the_device_wants() {
    let mock = Mock::new(48_000, 2);
    let (mixer, handle) = mixer_with_tone();
    let output = AudioOutput::with_backend(mock.clone(), mixer, &DeviceConfig::default()).unwrap();
    let info = output.info().unwrap().clone();
    assert!(!info.resampled);
    assert_eq!(
        info.buffer_latency,
        Some(Duration::from_secs_f64(512.0 / 48_000.0))
    );
    for frames in [512, 441, 1, 3_000] {
        mock.pull(frames);
    }
    assert_eq!(handle.frame(), 512 + 441 + 1 + 3_000);
}

#[test]
fn a_device_at_another_rate_is_resampled_and_keeps_the_sound() {
    let mock = Mock::new(44_100, 2);
    let (mixer, handle) = mixer_with_tone();
    let output = AudioOutput::with_backend(mock.clone(), mixer, &DeviceConfig::default()).unwrap();
    assert!(output.info().unwrap().resampled);
    let mut pulled = 0;
    let mut last = Vec::new();
    for _ in 0..20 {
        last = mock.pull(441);
        pulled += 441;
    }
    // The mixer ran at its own rate: 48 kHz for the device's 44.1 kHz, plus the frames
    // that the resampler renders ahead.
    let expected = pulled as u64 * 48_000 / 44_100;
    assert!(
        (expected..expected + 2_100).contains(&handle.frame()),
        "{}",
        handle.frame()
    );
    // A centred mono voice at 0.25 is 0.25 · cos(π/4) on each side.
    for sample in &last {
        assert!(
            (sample - 0.25 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3,
            "{sample}"
        );
    }
}

#[test]
fn a_sample_rate_change_reopens_the_stream_and_keeps_the_voices() {
    let mock = Mock::new(48_000, 2);
    let (mixer, mut handle) = mixer_with_tone();
    let mut output =
        AudioOutput::with_backend(mock.clone(), mixer, &DeviceConfig::default()).unwrap();
    mock.pull(4_800);
    mock.set_rate(44_100);
    mock.report(BackendEvent::Lost(Error::new(
        ErrorCode::DeviceUnavailable,
        "invalidated",
    )));
    let events = drain(&mut output);
    let [DeviceEvent::Reopened(info)] = events.as_slice() else {
        panic!("{events:?}");
    };
    assert_eq!((info.sample_rate, info.resampled), (44_100, true));
    assert_eq!(mock.starts(), 2);
    // The voice still plays, and never ended.
    for _ in 0..10 {
        mock.pull(441);
    }
    let samples = mock.pull(441);
    assert!(
        samples
            .iter()
            .all(|s| (s - 0.25 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3)
    );
    assert_eq!(handle.poll_event(), None);
}

#[test]
fn the_callback_allocates_nothing() {
    for rate in [48_000, 44_100] {
        let mock = Mock::new(rate, 2);
        let (mixer, _handle) = mixer_with_tone();
        let _output =
            AudioOutput::with_backend(mock.clone(), mixer, &DeviceConfig::default()).unwrap();
        mock.pull(512);
        for frames in [512, 441, 2_048] {
            assert_eq!(
                mock.allocations_in_pull(frames),
                0,
                "{rate} Hz, {frames} frames"
            );
        }
    }
}

#[test]
fn a_new_default_device_is_followed() {
    let mock = Mock::new(48_000, 2);
    let (mixer, _handle) = mixer_with_tone();
    let mut output =
        AudioOutput::with_backend(mock.clone(), mixer, &DeviceConfig::default()).unwrap();
    assert!(drain(&mut output).is_empty());
    mock.0.lock().unwrap().default_id = "headphones".into();
    // The output checks the default device once a second.
    std::thread::sleep(Duration::from_millis(1_100));
    let events = drain(&mut output);
    let [DeviceEvent::Reopened(info)] = events.as_slice() else {
        panic!("{events:?}");
    };
    assert_eq!(info.device_id, "headphones");
}

#[test]
fn a_named_device_that_disappears_falls_back_to_the_default() {
    let mock = Mock::new(48_000, 2);
    let (mixer, _handle) = mixer_with_tone();
    let config = DeviceConfig {
        device: Some("usb".into()),
        ..DeviceConfig::default()
    };
    let mut output = AudioOutput::with_backend(mock.clone(), mixer, &config).unwrap();
    assert_eq!(output.info().unwrap().device_name, "USB headset");
    mock.0.lock().unwrap().named_present = false;
    mock.report(BackendEvent::Lost(Error::new(
        ErrorCode::DeviceUnavailable,
        "unplugged",
    )));
    let events = drain(&mut output);
    assert!(
        matches!(&events[0], DeviceEvent::FellBackToDefault(error) if error.code == ErrorCode::DeviceNotFound),
        "{events:?}"
    );
    assert!(
        matches!(&events[1], DeviceEvent::Reopened(info) if info.is_default),
        "{events:?}"
    );
}

#[test]
fn rerouting_and_warnings_are_reported_without_reopening() {
    let mock = Mock::new(48_000, 2);
    let (mixer, _handle) = mixer_with_tone();
    let mut output =
        AudioOutput::with_backend(mock.clone(), mixer, &DeviceConfig::default()).unwrap();
    mock.report(BackendEvent::Rerouted);
    mock.report(BackendEvent::Warning(Error::new(
        ErrorCode::StreamFailed,
        "underrun",
    )));
    let events = drain(&mut output);
    assert!(
        matches!(
            events.as_slice(),
            [DeviceEvent::Rerouted, DeviceEvent::Warning(_)]
        ),
        "{events:?}"
    );
    assert_eq!(mock.starts(), 1);
}

#[test]
fn a_buffer_size_out_of_range_is_rejected() {
    let (mixer, _handle) = mixer_with_tone();
    let config = DeviceConfig {
        buffer_frames: Some(8),
        ..DeviceConfig::default()
    };
    let error = AudioOutput::with_backend(Mock::new(48_000, 2), mixer, &config)
        .err()
        .unwrap();
    assert_eq!(error.code, ErrorCode::InvalidArgument);
}

#[test]
fn a_surround_device_gets_the_mixer_on_its_first_two_channels() {
    let mock = Mock::new(48_000, 6);
    let (mixer, _handle) = mixer_with_tone();
    let _output = AudioOutput::with_backend(mock.clone(), mixer, &DeviceConfig::default()).unwrap();
    let samples = mock.pull(256);
    for frame in samples.as_chunks::<6>().0 {
        assert!((frame[0] - 0.25 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
        assert_eq!(frame[0], frame[1]);
        assert!(frame[2..].iter().all(|s| *s == 0.0));
    }
}

#[test]
fn the_player_loads_sounds_and_plays_them_by_id() {
    use audiobox::mixer::Fade;
    use audiobox::synth::RenderOptions;
    use audiobox_device::{PlayOptions, PlayerEvent};

    let mock = Mock::new(48_000, 2);
    let config = MixerConfig {
        buses: vec!["music".into(), "effects".into()],
        ..MixerConfig::default()
    };
    let (_output, mut player) =
        AudioOutput::with_backend_player(mock.clone(), &config, &DeviceConfig::default()).unwrap();
    assert_eq!(player.sample_rate(), 48_000);
    let effects = player.bus("effects").unwrap();
    assert!(player.bus("voices").is_none());

    let click = player
        .load(Audio::new(vec![vec![0.5; 240]], 48_000).unwrap())
        .unwrap();
    let voice = player
        .play(
            click,
            &PlayOptions {
                bus: effects,
                ..PlayOptions::default()
            },
        )
        .unwrap();
    let first = mock.pull(128);
    assert!(
        (first[0] - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4,
        "{}",
        first[0]
    );
    mock.pull(256);
    assert_eq!(player.poll_event(), Some(PlayerEvent::Ended(voice)));
    assert_eq!(player.frame(), 384);

    // A bus at zero silences what plays through it.
    player.set_bus_gain(effects, 0.0, Fade::NONE).unwrap();
    player
        .play(
            click,
            &PlayOptions {
                bus: effects,
                ..PlayOptions::default()
            },
        )
        .unwrap();
    assert!(mock.pull(128).iter().all(|s| *s == 0.0));

    // A score, checked when it is loaded, plays as it is synthesized.
    let bad = player.load_score("channel x = @nothing c", RenderOptions::default());
    assert_eq!(
        bad.err().map(|error| error.code),
        Some(ErrorCode::InvalidArgument)
    );
    let song = player
        .load_score(
            "tempo 150\ninstrument lead wave=pulse duty=25\nchannel lead = @lead o5 l8 c e g e",
            RenderOptions::default(),
        )
        .unwrap();
    let music = player.play(song, &PlayOptions::default()).unwrap();
    let energy: f32 = (0..20).flat_map(|_| mock.pull(128)).map(|s| s * s).sum();
    assert!(energy > 0.1, "{energy}");
    // Scheduled at an exact frame.
    let at = player.frame() + 1_000;
    player.at(at).stop(music, Fade::NONE).unwrap();
    mock.pull(1_200);
    assert!(
        std::iter::from_fn(|| player.poll_event()).any(|event| event == PlayerEvent::Ended(music))
    );
}

#[test]
fn a_suspended_output_stops_its_stream_and_resumes_the_voices() {
    let mock = Mock::new(48_000, 2);
    let (mixer, mut handle) = mixer_with_tone();
    let mut output =
        AudioOutput::with_backend(mock.clone(), mixer, &DeviceConfig::default()).unwrap();
    mock.pull(4_800);
    output.suspend();
    output.suspend();
    assert!(output.is_suspended());
    assert!(output.info().is_none());
    // A loss reported by the stopped stream, or a new default device, reopens nothing
    // while the output is suspended.
    mock.report(BackendEvent::Lost(Error::new(
        ErrorCode::DeviceUnavailable,
        "gone",
    )));
    mock.0.lock().unwrap().default_id = "headphones".into();
    std::thread::sleep(Duration::from_millis(1_100));
    assert!(drain(&mut output).is_empty());
    assert_eq!(mock.starts(), 1);
    // Resumed, on the default device, and the voice goes on.
    let info = output.resume().unwrap().clone();
    assert!(!output.is_suspended());
    assert_eq!(info.device_id, "headphones");
    assert_eq!(mock.starts(), 2);
    let events = drain(&mut output);
    assert!(
        matches!(events.as_slice(), [DeviceEvent::Reopened(_)]),
        "{events:?}"
    );
    let samples = mock.pull(480);
    assert!(
        samples
            .iter()
            .all(|s| (s - 0.25 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3)
    );
    assert_eq!(handle.poll_event(), None);
}
