# audiobox-device

Plays [audiobox](https://github.com/jmsansan/audiobox-rs)'s real-time mixer on an audio
device, and keeps it playing when the device changes.

`audiobox` makes, mixes and processes the samples, with no dependencies and no `unsafe`.
This sibling crate carries them to the speakers: it opens the operating system's output
stream with [cpal](https://crates.io/crates/cpal) and calls the mixer from the device's
audio thread.

```toml
[dependencies]
audiobox = { version = "0.2", features = ["mixer"] }
audiobox-device = "0.1"
```

```rust
use audiobox::mixer::{Mixer, MixerConfig, Play};
use audiobox_device::{AudioOutput, DeviceConfig};

let (mixer, mut handle) = Mixer::new(MixerConfig::default())?;
let mut output = AudioOutput::open(mixer, &DeviceConfig::default())?;

// In the game: start voices with the handle...
handle.play(sound, &Play::default())?;
// ...and poll the output once per frame.
while let Some(event) = output.poll() {
    println!("{event:?}");
}
```

## What it does

| | |
| --- | --- |
| Configuration | Opens the default output device, or one by id (`AudioOutput::devices`), preferring the mixer's sample rate, at least its channels and `f32` samples, and converting to whatever the device takes |
| Sample rates | The mixer always renders at its own rate. A device at another rate gets its frames through a 16-tap windowed-sinc resampler, so voices, fades and scheduled changes keep their timing |
| Channels | A stereo mixer plays on the first two channels of a surround device and is averaged on a mono one; samples are clamped to -1..=1 |
| Device changes | `AudioOutput::poll` reopens the stream when the device disappears, its configuration changes, or the system's default device changes, and the mixer keeps its voices. A named device that disappears falls back to the default one |
| Latency | `DeviceConfig::buffer_frames` (16 to 16,384) sets the frames per callback; `StreamInfo::buffer_latency` reports what one buffer adds. 256 to 1024 frames at 48 kHz (5 to 21 ms) suit most games |
| Real-time safety | The callback allocates nothing and never waits. The mixer is behind a mutex that only the callback locks, with `try_lock`; a buffer that cannot take it, which happens only while a stream is being replaced, is silent rather than late |
| Errors | `Error` with a stable `ErrorCode`: no device, device not found, unavailable, busy, permission denied, host unavailable, unsupported configuration, invalid argument, stream failed |
| Testing | `Backend` is a trait: tests drive the output with a simulated device, as `tests/mock.rs` does |

## Platforms

| Platform | Audio system | Status |
| --- | --- | --- |
| Windows | WASAPI | Ran on Windows 10, on a USB DAC and an HDMI monitor, with the examples |
| macOS | CoreAudio | Builds through `cpal`; not run yet |
| Linux | ALSA, and PulseAudio or PipeWire through it | Builds through `cpal`; not run yet |
| Android | AAudio | Experimental: not run on a device |
| iOS | CoreAudio | Experimental: not run on a device |
| Web | | Not in this crate yet: a separate path ([#2](https://github.com/jmsansan/audiobox-device/issues/2)) |

## Examples

```sh
cargo run --example devices
cargo run --example play_score -- [seconds] [device id or "default"] [mixer rate]
```

`play_score` plays a looping chiptune score from `audiobox`'s synthesizer and a few
sound effects, polls the output as a game loop would, and prints how much audio the mixer
rendered against the time that passed.

## License

MIT
