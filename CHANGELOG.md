# Changelog

## 0.1.0 (unreleased)

- `AudioOutput` plays an `audiobox` mixer on the native audio device through `cpal`: device selection, sample-format conversion, resampling when the device runs at another rate, channel mapping, and reopening when the device disappears, changes configuration, or the default device changes.
- `Backend` trait, with `CpalBackend`, so tests and hosts can drive the output with a device of their own.
- Examples `devices` and `play_score`.
