# Changelog

## 0.1.0

- `AudioOutput` plays an `audiobox` mixer on the native audio device through `cpal`: device selection, sample-format conversion, resampling when the device runs at another rate, channel mapping, and reopening when the device disappears, changes configuration, or the default device changes.
- `WebOutput` plays the mixer in the browser, inside an `AudioWorklet` with a WebAssembly module of its own that ships in the crate; commands travel through the worklet's `MessagePort`.
- `Player`, one API for both: sounds and scores loaded once and played by id, voices, buses, scheduling at a frame, and events.
- A `Backend` trait, with `CpalBackend`, so tests and hosts can drive the native output with a device of their own.
- Examples `devices` and `play_score`, and `web-demo` with its browser check.
