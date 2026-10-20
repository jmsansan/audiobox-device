# audiobox-device

Plays [audiobox](https://github.com/jmsansan/audiobox-rs)'s real-time mixer on an audio
device: natively through [cpal](https://crates.io/crates/cpal), and in the browser
through an `AudioWorklet`, with one API for both.

`audiobox` makes, mixes and processes the samples, with no dependencies and no `unsafe`.
This sibling crate carries them to the speakers, and keeps them playing when the device
changes.

```toml
[dependencies]
audiobox = { version = "0.2", features = ["mixer", "synth"] }
audiobox-device = "0.1"
```

## One API, native and Web

A `Player` loads sounds and plays them by id. A game writes the same calls for every
platform; only opening the output differs.

```rust
use audiobox::mixer::MixerConfig;
use audiobox::synth::RenderOptions;
use audiobox_device::{PlayOptions, Player};

fn level_starts(player: &mut Player) -> audiobox_device::Result<()> {
    let music = player.bus("music").unwrap();
    let song = player.load_score(SONG, RenderOptions { loops: None, ..RenderOptions::default() })?;
    player.play(song, &PlayOptions { bus: music, priority: 255, ..PlayOptions::default() })?;
    Ok(())
}

// Natively:
let config = MixerConfig { buses: vec!["music".into(), "effects".into()], ..MixerConfig::default() };
let (mut output, mut player) = audiobox_device::AudioOutput::open_player(&config, &Default::default())?;

// In the browser (wasm32-unknown-unknown):
let (output, mut player) = audiobox_device::WebOutput::open(&config).await?;
output.resume(); // from a user gesture's handler
```

- `load(Audio)` loads decoded audio, mono or stereo, once.
- `load_score(text, options)` loads a score of `audiobox`'s synthesizer, which plays as it
  is synthesized. In the browser only its text travels to the audio thread: a song is a
  few kilobytes.
- `play`, `stop`, `stop_all`, `set_gain`, `set_pan`, `set_bus_gain`, and the same at an
  exact frame through `at(frame)`.
- `poll_event` reports voices that ended, were stolen or rejected, and, in the browser,
  commands that failed on the audio thread.

Natively, `AudioOutput::open(mixer, &config)` also takes an `audiobox` `Mixer` directly,
for a game that drives its `MixerHandle` itself.

## Natively

| | |
| --- | --- |
| Configuration | Opens the default output device, or one by id (`AudioOutput::devices`), preferring the mixer's sample rate, at least its channels and `f32` samples, and converting to whatever the device takes |
| Sample rates | The mixer always renders at its own rate. A device at another rate gets its frames through a 16-tap windowed-sinc resampler, so voices, fades and scheduled changes keep their timing |
| Channels | A stereo mixer plays on the first two channels of a surround device and is averaged on a mono one; samples are clamped to -1..=1 |
| Device changes | `AudioOutput::poll` reopens the stream when the device disappears, its configuration changes, or the system's default device changes, and the mixer keeps its voices. A named device that disappears falls back to the default one |
| Latency | `DeviceConfig::buffer_frames` (16 to 16,384) sets the frames per callback; `StreamInfo::buffer_latency` reports what one buffer adds. 256 to 1024 frames at 48 kHz (5 to 21 ms) suit most games |
| Real-time safety | The callback allocates nothing and never waits. The mixer is behind a mutex that only the callback locks, with `try_lock`; a buffer that cannot take it, which happens only while a stream is being replaced, is silent rather than late |
| Testing | `Backend` is a trait: tests drive the output with a simulated device, as `tests/mock.rs` does |

## In the browser

The browser's audio thread cannot call into the page's WebAssembly, so the mixer runs
inside an `AudioWorklet`, in a small WebAssembly module of its own, and the page's `Player`
sends it commands through the worklet's `MessagePort`.

- **Nothing to serve.** The module (about 250 KB, no imports) and the worklet's script ship
  inside this crate; the script is loaded from a blob URL and the module is compiled by the
  page and handed to the worklet.
- **No special headers.** Commands travel as small binary messages, not through a
  `SharedArrayBuffer`, so the page needs no cross-origin isolation and works on any host
  and inside iframes. A command applies at the start of the worklet's next quantum: about
  2.7 ms at 48 kHz, plus a message's delivery.
- **No deprecated APIs.** No `ScriptProcessorNode`.
- **Starting.** Browsers start audio only after a user gesture: call `WebOutput::resume`
  from an input handler; `is_running` tells whether it plays.
- **The one piece of JavaScript** is `web/worklet.js`, about 60 lines: it instantiates the
  module, copies messages into its memory, and copies each rendered quantum to the
  outputs. Everything else is Rust.

`web-demo/` is a page that plays music and effects through `WebOutput`, and
`node web-demo/check.mjs` builds it, opens it in a headless Chromium-based browser and
checks that audio runs, the worklet renders, an analyser hears the music, effects end,
and commands keep their order while the page is busy.

## Platforms

| Platform | Audio system | Status |
| --- | --- | --- |
| Windows | WASAPI | Ran on Windows 10, on a USB DAC and an HDMI monitor |
| Web | AudioWorklet | Ran in Edge 155 and Brave on Windows, headless, with `web-demo/check.mjs`, and in Edge 155 on macOS, headless and in a window |
| macOS | CoreAudio | Ran on macOS 26.6 on an M2 Pro, on the built-in speakers at 48 kHz and Bluetooth headphones at 44.1 kHz, resampled and not |
| Linux | ALSA, and PulseAudio or PipeWire through it | Builds through `cpal`; not run yet |
| Android | AAudio | Experimental: not run on a device |
| iOS | CoreAudio | Experimental: not run on a device |

Firefox and Safari have not been tried.

## Examples and development

```sh
cargo run --example devices
cargo run --example play_score -- [seconds] [device id or "default"] [mixer rate]
node web-demo/check.mjs
```

The worklet's module is built from `worklet/` by `node scripts/build-worklet.mjs` into
`web/audiobox_device_worklet.wasm`, which is committed and embedded;
`node scripts/build-worklet.mjs --check` verifies that it matches the source. The
repository pins Rust 1.98.1 with the `wasm32-unknown-unknown` target in
`rust-toolchain.toml`. There is no CI on pushes or pull requests; validation is local.

## License

MIT
