//! Lists the output devices, with the id that `DeviceConfig::device` takes.
//!
//! ```sh
//! cargo run --example devices
//! ```
use audiobox_device::AudioOutput;

fn main() -> Result<(), audiobox_device::Error> {
    for device in AudioOutput::devices()? {
        let default = if device.is_default { " (default)" } else { "" };
        println!("{}{default}\n  id: {}", device.name, device.id);
    }
    Ok(())
}
