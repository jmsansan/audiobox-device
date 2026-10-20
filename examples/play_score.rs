//! A game's audio on the audio device: looping chiptune music on a music bus and sound
//! effects on an effects bus, for a few seconds, with the output polled as a game loop
//! would.
//!
//! ```sh
//! cargo run --example play_score -- [seconds] [device id or "default"] [mixer rate]
//! ```
use std::sync::Arc;
use std::time::{Duration, Instant};

use audiobox::mixer::{Fade, Mixer, MixerConfig, Play};
use audiobox::sfx::{Effect, Preset};
use audiobox::synth::{RenderOptions, Score, Synth};
use audiobox_device::{AudioOutput, DeviceConfig};

const SONG: &str = "
tempo 150
instrument lead wave=pulse duty=25 adsr=4,80,60,60 vibrato=12,170,200
instrument bass wave=triangle adsr=2,0,100,25
channel lead = @lead o5 l8 q7 L [c e g e >c< g e g]2 [d f a f >d< a f a]2
channel bass = @bass o2 l4 L [c c g g]2 [d d a a]2
";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let seconds: f64 = args.next().map_or(Ok(6.0), |text| text.parse())?;
    let device = args.next().filter(|id| id != "default");
    let rate: u32 = args.next().map_or(Ok(48_000), |text| text.parse())?;

    let (mixer, mut audio) = Mixer::new(MixerConfig {
        sample_rate: rate,
        buses: vec!["music".into(), "effects".into()],
        ..MixerConfig::default()
    })?;
    let music = audio.bus("music").unwrap();
    let effects = audio.bus("effects").unwrap();
    let jump = Arc::new(Effect::preset(Preset::Jump, 7).render(rate)?);
    let coin = Arc::new(Effect::preset(Preset::Pickup, 3).render(rate)?);
    let score: Score = SONG.parse()?;
    let song = Synth::new(
        &score,
        rate,
        &RenderOptions {
            loops: None,
            ..RenderOptions::default()
        },
    )?;

    let mut output = AudioOutput::open(
        mixer,
        &DeviceConfig {
            device,
            buffer_frames: Some(512),
        },
    )?;
    let info = output.info().unwrap();
    println!(
        "Playing on {} ({}): {} Hz, {} channels, {}, buffer {:?} frames, resampled: {}",
        info.device_name,
        info.device_id,
        info.sample_rate,
        info.channels,
        info.sample_format,
        info.buffer_frames,
        info.resampled
    );

    audio.play(
        song,
        &Play {
            bus: music,
            priority: 255,
            fade_in: Fade::linear(1.0),
            ..Play::default()
        },
    )?;
    let start = Instant::now();
    let mut next_effect = 1.0;
    let mut effect = 0;
    while start.elapsed().as_secs_f64() < seconds {
        // A game frame: an effect now and then, then the output's and mixer's events.
        if start.elapsed().as_secs_f64() >= next_effect {
            let sound = if effect % 2 == 0 {
                Arc::clone(&jump)
            } else {
                Arc::clone(&coin)
            };
            let pan = if effect % 2 == 0 { -0.5 } else { 0.5 };
            audio.play(
                sound,
                &Play {
                    bus: effects,
                    pan,
                    ..Play::default()
                },
            )?;
            effect += 1;
            next_effect += 0.75;
        }
        while let Some(event) = output.poll() {
            println!("device: {event:?}");
        }
        while audio.poll_event().is_some() {}
        std::thread::sleep(Duration::from_millis(16));
    }
    let played = audio.frame() as f64 / f64::from(rate);
    println!(
        "The mixer rendered {played:.2} s of audio in {:.2} s.",
        start.elapsed().as_secs_f64()
    );
    Ok(())
}
