# Changelog

## 0.2.1

- `AudioOutput::suspend` stops the device's stream for an application that the system hides, such as an Android activity in the background, and `resume` opens it again; the mixer's voices keep their place. `is_suspended` tells which. `poll` reopens nothing while the output is suspended.

## 0.2.0

Built on `audiobox` 0.3, whose controls a `Player` now carries, natively and in the browser.

- `PlayOptions` has a voice's `rate`, `pitch`, seeded `variation`, `start`, `instances` limits and 2D `position`. `SoundId::instances()` keys the limits by the loaded sound.
- `Player::set_rate`, `set_pitch`, `set_position`, `pause`, `resume`, `pause_bus`, `resume_bus` and `reseed`, and the same at a frame through `Player::at`.
- `PlayerEvent::Limited` reports a start that a sound's instance limits refused. `PlayerEvent` is `#[non_exhaustive]`.
- The browser's mixer is made with the configuration's `seed`, `space` and `limiter`.

### Breaking

- `audiobox` 0.3, whose types are in the API.
- `PlayOptions` has new fields: build it with `..PlayOptions::default()`.
- A `match` on `PlayerEvent` needs a wildcard arm.
- The worklet's protocol is version 2. A page and a worklet of different versions fail clearly, and both ship in this crate.

## 0.1.0

- `AudioOutput` plays an `audiobox` mixer on the native audio device through `cpal`: device selection, sample-format conversion, resampling when the device runs at another rate, channel mapping, and reopening when the device disappears, changes configuration, or the default device changes.
- `WebOutput` plays the mixer in the browser, inside an `AudioWorklet` with a WebAssembly module of its own that ships in the crate; commands travel through the worklet's `MessagePort`.
- `Player`, one API for both: sounds and scores loaded once and played by id, voices, buses, scheduling at a frame, and events.
- A `Backend` trait, with `CpalBackend`, so tests and hosts can drive the native output with a device of their own.
- Examples `devices` and `play_score`, and `web-demo` with its browser check.
